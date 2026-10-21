//! Freehand strokes and the smooth path drawn through their points.

use eframe::egui::{Color32, Pos2};

/// One freehand stroke. Points are in PDF user space, so the stroke stays on the same spot of the
/// page at any zoom and is written to the file exactly as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// The page the stroke was started on (0-based).
    pub page: usize,
    /// Pointer positions in PDF user space (pt).
    pub points: Vec<Pos2>,
    pub color: Color32,
    /// Line width in pt.
    pub width: f32,
}

/// One piece of a stroke's path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    MoveTo(Pos2),
    LineTo(Pos2),
    /// A cubic Bézier curve: first control point, second control point, end point.
    CubicTo(Pos2, Pos2, Pos2),
}

impl Stroke {
    /// A stroke without points yet.
    pub fn new(page: usize, color: Color32, width: f32) -> Stroke {
        Stroke {
            page,
            points: Vec::new(),
            color,
            width,
        }
    }

    /// Adds a pointer position, unless it is closer than `min_distance` (pt) to the last one kept.
    /// This drops the duplicate and sub-pixel samples a trackpad produces.
    pub fn push(&mut self, p: Pos2, min_distance: f32) {
        if self
            .points
            .last()
            .is_none_or(|last| last.distance(p) >= min_distance)
        {
            self.points.push(p);
        }
    }

    /// The smoothed path. Screen painting and the saved PDF both use it, so they match.
    ///
    /// It is a Catmull-Rom spline written as cubic Béziers, which passes through every point
    /// (the ink goes where the pen went) and needs no tuning. A single point becomes a
    /// zero-length line, which round line caps draw as a dot.
    pub fn segments(&self) -> Vec<Segment> {
        let p = &self.points;
        let Some(&first) = p.first() else {
            return Vec::new();
        };
        let mut segments = vec![Segment::MoveTo(first)];
        if p.len() == 1 {
            segments.push(Segment::LineTo(first));
            return segments;
        }
        let last = p.len() - 1;
        for i in 0..last {
            // The curve's direction at a point is parallel to the line between its neighbours.
            // The ends use themselves as their missing neighbour.
            let before = p[i.saturating_sub(1)];
            let after = p[(i + 2).min(last)];
            let c1 = p[i] + (p[i + 1] - before) / 6.0;
            let c2 = p[i + 1] - (after - p[i]) / 6.0;
            segments.push(Segment::CubicTo(c1, c2, p[i + 1]));
        }
        segments
    }

    /// The smoothed path as a polyline: curves are cut into straight pieces at most about
    /// `step` long (in pt). A dot gives its point twice.
    pub fn polyline(&self, step: f32) -> Vec<Pos2> {
        let mut out: Vec<Pos2> = Vec::new();
        for segment in self.segments() {
            match segment {
                Segment::MoveTo(p) | Segment::LineTo(p) => out.push(p),
                Segment::CubicTo(c1, c2, end) => {
                    let start = out.last().copied().unwrap_or(c1);
                    // A cubic Bézier never moves faster than 3 × its longest control leg per unit
                    // of t, so this many equal steps in t keep every piece within `step`. The
                    // upper bound only guards against a zero or tiny `step`.
                    let leg = start
                        .distance(c1)
                        .max(c1.distance(c2))
                        .max(c2.distance(end));
                    let pieces = ((3.0 * leg / step).ceil() as usize).clamp(1, 1000);
                    for i in 1..=pieces {
                        let t = i as f32 / pieces as f32;
                        out.push(cubic_point(start, c1, c2, end, t));
                    }
                }
            }
        }
        out
    }

    /// Whether the stroke's ink comes within `radius` (pt) of `p`.
    pub fn hit(&self, p: Pos2, radius: f32) -> bool {
        let reach = radius + self.width / 2.0;
        let path = self.polyline((reach / 2.0).max(0.1));
        path.windows(2)
            .any(|piece| distance_to_segment(p, piece[0], piece[1]) <= reach)
    }
}

/// Points from `a` to `b` (both included) at most `spacing` apart.
pub fn points_along(a: Pos2, b: Pos2, spacing: f32) -> impl Iterator<Item = Pos2> {
    let steps = ((a.distance(b) / spacing).ceil() as usize).max(1);
    (0..=steps).map(move |i| a.lerp(b, i as f32 / steps as f32))
}

/// The point at `t` (0..=1) on the cubic Bézier curve `p0`, `c1`, `c2`, `p3`.
fn cubic_point(p0: Pos2, c1: Pos2, c2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let sum = p0.to_vec2() * (u * u * u)
        + c1.to_vec2() * (3.0 * u * u * t)
        + c2.to_vec2() * (3.0 * u * t * t)
        + p3.to_vec2() * (t * t * t);
    sum.to_pos2()
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn stroke(points: &[Pos2]) -> Stroke {
        Stroke {
            page: 0,
            points: points.to_vec(),
            color: Color32::BLACK,
            width: 2.0,
        }
    }

    #[test]
    fn push_drops_points_that_are_too_close() {
        let mut s = Stroke::new(3, Color32::RED, 1.5);
        for p in [
            pos2(0.0, 0.0),
            pos2(0.0, 0.0),
            pos2(0.3, 0.0),
            pos2(1.0, 0.0),
            pos2(1.2, 0.2),
            pos2(5.0, 5.0),
        ] {
            s.push(p, 0.5);
        }
        assert_eq!(s.points, [pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(5.0, 5.0)]);
        assert_eq!((s.page, s.color, s.width), (3, Color32::RED, 1.5));
    }

    #[test]
    fn smoothing_passes_through_every_point() {
        let points = [
            pos2(10.0, 10.0),
            pos2(30.0, 40.0),
            pos2(60.0, 35.0),
            pos2(61.0, 80.0),
            pos2(20.0, 90.0),
        ];
        let segments = stroke(&points).segments();
        assert_eq!(segments.len(), points.len());
        assert_eq!(segments[0], Segment::MoveTo(points[0]));
        for (segment, point) in segments[1..].iter().zip(&points[1..]) {
            let Segment::CubicTo(_, _, end) = *segment else {
                panic!("expected a curve, got {segment:?}");
            };
            assert_eq!(end, *point);
        }
        // The polyline keeps every input point too.
        let line = stroke(&points).polyline(1.0);
        for p in points {
            assert!(line.contains(&p), "{p:?} missing");
        }
    }

    #[test]
    fn curve_leaves_a_point_parallel_to_its_neighbours() {
        let s = stroke(&[pos2(0.0, 0.0), pos2(10.0, 10.0), pos2(20.0, 0.0)]);
        let segments = s.segments();
        let (Segment::CubicTo(_, before, _), Segment::CubicTo(after, _, _)) =
            (segments[1], segments[2])
        else {
            panic!("{segments:?}");
        };
        // Smooth at the middle point: the handles on both sides are horizontal and opposite.
        assert_eq!(before, pos2(10.0 - 20.0 / 6.0, 10.0));
        assert_eq!(after, pos2(10.0 + 20.0 / 6.0, 10.0));
    }

    #[test]
    fn a_single_point_is_a_dot() {
        let p = pos2(100.0, 200.0);
        assert_eq!(
            stroke(&[p]).segments(),
            [Segment::MoveTo(p), Segment::LineTo(p)]
        );
        assert_eq!(stroke(&[p]).polyline(1.0), [p, p]);
        assert_eq!(stroke(&[]).segments(), []);
        assert_eq!(stroke(&[]).polyline(1.0), []);
    }

    #[test]
    fn polyline_pieces_are_short() {
        let s = stroke(&[pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(100.0, 100.0)]);
        let line = s.polyline(2.0);
        assert!(line.windows(2).all(|w| w[0].distance(w[1]) <= 2.0));
        assert_eq!(line.first(), Some(&pos2(0.0, 0.0)));
        assert_eq!(line.last(), Some(&pos2(100.0, 100.0)));
    }

    #[test]
    fn points_along_include_both_ends_and_are_close() {
        let points: Vec<Pos2> = points_along(pos2(0.0, 0.0), pos2(10.0, 0.0), 3.0).collect();
        assert_eq!(points.first(), Some(&pos2(0.0, 0.0)));
        assert_eq!(points.last(), Some(&pos2(10.0, 0.0)));
        assert!(points.windows(2).all(|w| w[0].distance(w[1]) <= 3.0));
        let same: Vec<Pos2> = points_along(pos2(1.0, 1.0), pos2(1.0, 1.0), 1.0).collect();
        assert_eq!(same, [pos2(1.0, 1.0), pos2(1.0, 1.0)]);
    }

    #[test]
    fn hit_counts_the_radius_and_half_the_width() {
        // Width 2: the ink reaches 1 pt to each side of the centre line.
        let s = stroke(&[pos2(0.0, 0.0), pos2(100.0, 0.0)]);
        assert!(s.hit(pos2(50.0, 3.9), 3.0));
        assert!(!s.hit(pos2(50.0, 4.1), 3.0));
        assert!(s.hit(pos2(-3.9, 0.0), 3.0)); // round end
        assert!(!s.hit(pos2(-4.1, 0.0), 3.0));
        let dot = stroke(&[pos2(10.0, 10.0)]);
        assert!(dot.hit(pos2(12.0, 12.0), 2.0));
        assert!(!dot.hit(pos2(13.0, 13.0), 2.0));
        assert!(!stroke(&[]).hit(pos2(0.0, 0.0), 100.0));
    }
}
