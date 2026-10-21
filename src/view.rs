//! Layout and zoom math for the scrolling page view. It only works on plain numbers, so it can be
//! tested without a window.
//!
//! "Content space" is the scroll area's content: (0, 0) is its top-left corner, and the scroll
//! offset is the content-space point shown at the top-left corner of the viewport.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};
use std::ops::Range;

/// Space around and between pages, in screen points. It does not scale with the zoom.
pub const GAP: f32 = 16.0;
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 4.0;
/// Longest side of a page bitmap, whatever the GPU allows: 6144 × 6144 RGBA is already 150 MB.
pub const MAX_RENDER_SIDE: usize = 6144;

/// Content-space rects of the pages at `zoom`, stacked top to bottom and centred, plus the
/// content size. Content narrower than `min_width` (pass the viewport width) is centred in it,
/// because the scroll area itself would keep it at the left.
pub fn layout(page_sizes: &[Vec2], zoom: f32, min_width: f32) -> (Vec<Rect>, Vec2) {
    let widest = page_sizes.iter().map(|s| s.x).fold(0.0, f32::max) * zoom;
    let width = (widest + 2.0 * GAP).max(min_width);
    let mut y = GAP;
    let mut rects = Vec::with_capacity(page_sizes.len());
    for &size in page_sizes {
        let size = size * zoom;
        rects.push(Rect::from_min_size(pos2((width - size.x) / 2.0, y), size));
        y += size.y + GAP;
    }
    (rects, vec2(width, y))
}

/// The page whose vertical centre is nearest to content-space height `y`.
pub fn nearest_page(rects: &[Rect], y: f32) -> Option<usize> {
    (0..rects.len()).min_by(|&i, &j| {
        let distance = |k: usize| (rects[k].center().y - y).abs();
        distance(i).total_cmp(&distance(j))
    })
}

/// The scroll offset that keeps the page point under `pointer` where it is on screen while the
/// zoom changes from `old_zoom` to `new_zoom`.
///
/// `offset` must be the offset the current frame was drawn with (`viewport.min` in
/// `ScrollArea::show_viewport`), not the scroll state's offset, which may already include this
/// frame's scrolling. `inner` is the scroll area's screen rect. Anchoring goes through the page
/// under the pointer because the gaps between pages do not scale. The result is clamped to the
/// scrollable range, so it is exact only while the needed offset fits in that range.
pub fn zoom_about(
    page_sizes: &[Vec2],
    old_zoom: f32,
    new_zoom: f32,
    offset: Vec2,
    pointer: Pos2,
    inner: Rect,
) -> Vec2 {
    let local = pointer - inner.min;
    let content = offset + local;
    let (old, _) = layout(page_sizes, old_zoom, inner.width());
    let Some(i) = nearest_page(&old, content.y) else {
        return offset;
    };
    let page_point = (content - old[i].min.to_vec2()) / old_zoom;
    let (new, size) = layout(page_sizes, new_zoom, inner.width());
    let wanted = new[i].min.to_vec2() + page_point * new_zoom - local;
    let max_offset = (size - inner.size()).max(Vec2::ZERO);
    wanted.min(max_offset).max(Vec2::ZERO)
}

/// Limits a zoom factor to the supported range (25 % to 400 %).
pub fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The zoom at which the widest page, with its gaps, fills `viewport_width`.
pub fn fit_width_zoom(page_sizes: &[Vec2], viewport_width: f32) -> f32 {
    let widest = page_sizes.iter().map(|s| s.x).fold(0.0, f32::max);
    if widest > 0.0 {
        clamp_zoom((viewport_width - 2.0 * GAP) / widest)
    } else {
        1.0
    }
}

/// Bitmap size in pixels for a page of `page_size` pt shown at `zoom` on a screen with
/// `pixels_per_point`, so it is sharp; the longest side is capped at
/// min(`max_texture_side`, [`MAX_RENDER_SIDE`]) (the GPU rejects larger textures).
pub fn render_size(
    page_size: Vec2,
    zoom: f32,
    pixels_per_point: f32,
    max_texture_side: usize,
) -> [usize; 2] {
    let pixels = page_size * zoom * pixels_per_point;
    let cap = max_texture_side.min(MAX_RENDER_SIDE) as f32;
    let scale = (cap / pixels.max_elem()).min(1.0);
    let side = |v: f32| ((v * scale).round() as usize).max(1);
    [side(pixels.x), side(pixels.y)]
}

/// The pages that overlap the content-space `viewport` (`rects` as returned by [`layout`]).
pub fn visible_pages(rects: &[Rect], viewport: Rect) -> Range<usize> {
    let first = rects.partition_point(|r| r.max.y < viewport.min.y);
    let end = rects.partition_point(|r| r.min.y <= viewport.max.y);
    first..end.max(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Vec2 = vec2(612.0, 792.0);
    const A4: Vec2 = vec2(595.0, 842.0);
    const LANDSCAPE: Vec2 = vec2(792.0, 612.0);

    #[test]
    fn layout_stacks_centres_and_keeps_gaps_unscaled() {
        let (rects, size) = layout(&[LETTER, A4], 2.0, 0.0);
        assert_eq!(rects[0], Rect::from_min_size(pos2(GAP, GAP), LETTER * 2.0));
        assert_eq!(
            rects[1],
            Rect::from_min_size(pos2(GAP + 17.0, GAP + 1584.0 + GAP), A4 * 2.0)
        );
        assert_eq!(size, vec2(1224.0 + 2.0 * GAP, 1584.0 + 1684.0 + 3.0 * GAP));
    }

    #[test]
    fn layout_centres_narrow_content_in_the_viewport() {
        let (rects, size) = layout(&[LETTER], 0.5, 1000.0);
        assert_eq!(size, vec2(1000.0, 396.0 + 2.0 * GAP));
        assert_eq!(rects[0].center().x, 500.0);
    }

    /// Where page `i`'s point `page_point` (pt from its top-left) is on screen.
    fn on_screen(
        pages: &[Vec2],
        zoom: f32,
        offset: Vec2,
        inner: Rect,
        i: usize,
        page_point: Vec2,
    ) -> Pos2 {
        let (rects, _) = layout(pages, zoom, inner.width());
        inner.min + rects[i].min.to_vec2() + page_point * zoom - offset
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let pages = [LETTER, A4, LANDSCAPE];
        let inner = Rect::from_min_size(pos2(0.0, 40.0), vec2(900.0, 700.0));
        // (old zoom, new zoom, scroll offset, page, point on that page in pt). The offsets are
        // reachable ones, and every needed offset fits the scrollable range.
        let cases = [
            (1.0, 2.0, vec2(0.0, 0.0), 0, vec2(300.0, 400.0)),
            (1.0, 1.5, vec2(0.0, 900.0), 1, vec2(100.0, 200.0)),
            (2.0, 1.1, vec2(630.0, 3300.0), 2, vec2(700.0, 50.0)),
            (1.5, 1.2, vec2(200.0, 1600.0), 1, vec2(400.0, 500.0)),
            (1.0, 4.0, vec2(0.0, 1600.0), 2, vec2(100.0, 600.0)),
        ];
        for (old_zoom, new_zoom, offset, page, point) in cases {
            let (_, size) = layout(&pages, old_zoom, inner.width());
            let max_offset = (size - inner.size()).max(Vec2::ZERO);
            assert!(
                offset.x <= max_offset.x && offset.y <= max_offset.y,
                "bad test case"
            );
            let pointer = on_screen(&pages, old_zoom, offset, inner, page, point);
            assert!(inner.contains(pointer), "bad test case {pointer:?}");
            let new_offset = zoom_about(&pages, old_zoom, new_zoom, offset, pointer, inner);
            let after = on_screen(&pages, new_zoom, new_offset, inner, page, point);
            // Within 1 pt of the page, i.e. `new_zoom` screen points.
            assert!(
                after.distance(pointer) <= new_zoom,
                "{old_zoom} -> {new_zoom}: {pointer:?} moved to {after:?}"
            );
        }
    }

    #[test]
    fn zoom_offset_stays_in_the_scrollable_range() {
        let pages = [LETTER, LETTER];
        let inner = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        // Zooming out near the top-left of the first page cannot scroll above the content.
        let offset = zoom_about(&pages, 1.0, 0.5, Vec2::ZERO, pos2(100.0, 20.0), inner);
        assert_eq!(offset, Vec2::ZERO);
        // Scrolled to the end, zooming out cannot scroll past the end.
        let (_, size) = layout(&pages, 1.0, inner.width());
        let end = vec2(0.0, size.y - inner.height());
        let offset = zoom_about(&pages, 1.0, 0.5, end, pos2(400.0, 10.0), inner);
        let (_, new_size) = layout(&pages, 0.5, inner.width());
        assert_eq!(offset, vec2(0.0, new_size.y - inner.height()));
        // No pages: nothing to anchor to.
        let offset = zoom_about(&[], 1.0, 2.0, vec2(3.0, 4.0), pos2(5.0, 5.0), inner);
        assert_eq!(offset, vec2(3.0, 4.0));
    }

    #[test]
    fn nearest_page_picks_by_vertical_centre() {
        let (rects, _) = layout(&[LETTER, LETTER, LETTER], 1.0, 0.0);
        assert_eq!(nearest_page(&rects, 0.0), Some(0));
        assert_eq!(nearest_page(&rects, rects[1].center().y), Some(1));
        assert_eq!(
            nearest_page(&rects, rects[1].max.y + GAP / 2.0 + 1.0),
            Some(2)
        );
        assert_eq!(nearest_page(&rects, 1e9), Some(2));
        assert_eq!(nearest_page(&[], 0.0), None);
    }

    #[test]
    fn zoom_is_clamped_and_fit_width_fills_the_viewport() {
        assert_eq!(clamp_zoom(0.1), MIN_ZOOM);
        assert_eq!(clamp_zoom(9.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(1.3), 1.3);
        let zoom = fit_width_zoom(&[LETTER, LANDSCAPE], 824.0);
        assert_eq!(zoom, 1.0); // the landscape page is 792 pt wide, plus 2 × 16 gap
        assert_eq!(fit_width_zoom(&[LETTER], 100_000.0), MAX_ZOOM);
        assert_eq!(fit_width_zoom(&[], 800.0), 1.0);
    }

    #[test]
    fn render_size_follows_zoom_and_screen_and_is_capped() {
        assert_eq!(render_size(LETTER, 1.0, 2.0, 8192), [1224, 1584]);
        assert_eq!(render_size(LETTER, 1.25, 2.0, 8192), [1530, 1980]);
        // 4× on a Retina screen would be 4896 × 6336: the long side is capped at 6144.
        assert_eq!(render_size(LETTER, 4.0, 2.0, 8192), [4748, 6144]);
        // The GPU limit wins when it is lower.
        assert_eq!(render_size(LANDSCAPE, 4.0, 2.0, 2048), [2048, 1583]);
        assert_eq!(render_size(vec2(1.0, 0.001), 0.25, 1.0, 8192), [1, 1]);
    }

    #[test]
    fn visible_pages_are_those_overlapping_the_viewport() {
        let (rects, _) = layout(&[LETTER, LETTER, LETTER], 1.0, 0.0);
        let viewport = |y0: f32, y1: f32| Rect::from_min_max(pos2(0.0, y0), pos2(600.0, y1));
        assert_eq!(visible_pages(&rects, viewport(0.0, 500.0)), 0..1);
        assert_eq!(visible_pages(&rects, viewport(700.0, 900.0)), 0..2);
        assert_eq!(visible_pages(&rects, viewport(0.0, 1e6)), 0..3);
        // Only the gap between page 1 and page 2 is visible.
        let gap = rects[0].max.y + 1.0;
        assert_eq!(visible_pages(&rects, viewport(gap, gap + 5.0)), 1..1);
        assert_eq!(visible_pages(&rects, viewport(1e6, 2e6)), 3..3);
    }
}
