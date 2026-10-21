//! A 2D affine transform, the kind PDF uses for its coordinate systems.

use eframe::egui::{Pos2, pos2};

/// A PDF-style 2×3 matrix: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    /// The transform that maps (0, 0) to `p0`, (1, 0) to `px` and (0, 1) to `py`.
    pub fn from_points(p0: Pos2, px: Pos2, py: Pos2) -> Affine {
        Affine {
            a: px.x - p0.x,
            b: px.y - p0.y,
            c: py.x - p0.x,
            d: py.y - p0.y,
            e: p0.x,
            f: p0.y,
        }
    }

    /// Transforms a point.
    pub fn apply(&self, p: Pos2) -> Pos2 {
        pos2(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// The transform that undoes this one, or `None` if this one squashes the plane onto a line.
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(p: Pos2, q: Pos2) {
        assert!(p.distance(q) < 1e-3, "{p:?} != {q:?}");
    }

    #[test]
    fn from_points_maps_the_unit_square_corners() {
        // A US Letter page with /Rotate 90: going right on screen goes up the page.
        let (p0, px, py) = (pos2(0.0, 0.0), pos2(0.0, 792.0), pos2(612.0, 0.0));
        let m = Affine::from_points(p0, px, py);
        assert_near(m.apply(pos2(0.0, 0.0)), p0);
        assert_near(m.apply(pos2(1.0, 0.0)), px);
        assert_near(m.apply(pos2(0.0, 1.0)), py);
        assert_near(m.apply(pos2(1.0, 1.0)), pos2(612.0, 792.0));
        assert_near(m.apply(pos2(0.5, 0.25)), pos2(153.0, 396.0));
    }

    #[test]
    fn inverse_undoes_apply() {
        let m = Affine::from_points(pos2(50.0, 742.0), pos2(562.0, 742.0), pos2(50.0, 100.0));
        let inv = m.inverse().unwrap();
        for p in [
            pos2(0.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.3, 0.7),
            pos2(-2.0, 5.0),
        ] {
            assert_near(inv.apply(m.apply(p)), p);
            assert_near(m.apply(inv.apply(p)), p);
        }
    }

    #[test]
    fn inverse_of_a_flat_transform_is_none() {
        let flat = Affine::from_points(pos2(0.0, 0.0), pos2(1.0, 1.0), pos2(2.0, 2.0));
        assert_eq!(flat.inverse(), None);
    }
}
