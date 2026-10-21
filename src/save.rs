//! Saving: draws the strokes into a new copy of the PDF, checks the copy, and writes it safely.
//!
//! The copy is built in memory from the original bytes and compared with the original before
//! anything is written, because PDFium can silently change a page while editing it (see
//! [`SaveError::ChangedOutsideDrawing`]).

use crate::ink::{Segment, Stroke, points_along};
use crate::pdf::{self, PageInfo};
use eframe::egui::{Color32, ColorImage, Pos2, pos2};
use pdfium_render::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// A pixel counts as changed when a colour channel differs by more than this.
const PIXEL_TOLERANCE: u8 = 16;
/// More changed pixels than this outside the ink means the page was damaged.
const CHANGED_PIXEL_ALLOWANCE: usize = 0;
/// A sample on a stroke's centre line is visible when every colour channel of the saved pixel is
/// within this of the stroke colour.
const INK_TOLERANCE: u8 = 40;
/// Longest side of the bitmaps compared by the check (two of them are in memory at once).
const MAX_CHECK_SIDE: f32 = 8192.0;
/// Half the side of the clip square that isolates the original content, in pt: far larger than
/// any page, so it never cuts anything off.
const ISOLATION_CLIP: f32 = 1.0e6;
/// Upper bound for the content wraps, so a hostile file cannot make saving take forever.
const MAX_WRAPS: usize = 16;
/// Upper bound for the decompressed content of one page when counting its q/Q operators.
const MAX_CONTENT_BYTES: usize = 256 << 20;

/// A new PDF, built in memory and checked.
#[derive(Debug)]
pub struct Built {
    /// The complete PDF file.
    pub bytes: Vec<u8>,
    /// Pages (0-based) where some of the ink would be hidden behind form fields or annotations,
    /// which PDF viewers draw on top of the page content.
    pub hidden_ink_pages: Vec<usize>,
}

/// Why no PDF was built.
#[derive(Debug)]
pub enum SaveError {
    /// The new copy of these pages (0-based) would look different outside the drawing. PDFium
    /// does this, for example, when it drops a page font that only an old-style form XObject
    /// uses. Nothing should be written.
    ChangedOutsideDrawing(Vec<usize>),
    /// PDFium failed.
    Pdf(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::ChangedOutsideDrawing(pages) => {
                let pages: Vec<String> = pages.iter().map(|i| (i + 1).to_string()).collect();
                write!(
                    f,
                    "Saving would change page(s) {} outside your drawing, so nothing was saved.",
                    pages.join(", ")
                )
            }
            SaveError::Pdf(why) => write!(f, "The PDF could not be saved: {why}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<PdfiumError> for SaveError {
    fn from(e: PdfiumError) -> Self {
        // PdfiumError's Display is its multi-line debug output; the one-line form reads better.
        SaveError::Pdf(format!("PDFium error {e:?}"))
    }
}

/// Builds the PDF with `strokes` drawn in and checks it.
///
/// `original` and `password` are what the document was opened with. Pages in `flatten`
/// (0-based) get their form fields and annotations merged into the page content first, so the
/// ink can be drawn on top of them.
pub fn build(
    original: &[u8],
    password: Option<&str>,
    strokes: &[Stroke],
    flatten: &BTreeSet<usize>,
) -> Result<Built, SaveError> {
    // See Document::open: a NUL in the password would poison PDFium for the whole process.
    if password.is_some_and(|p| p.contains('\0')) {
        return Err(SaveError::Pdf(
            "the password contains a NUL character".into(),
        ));
    }
    let pdfium = pdf::pdfium().map_err(SaveError::Pdf)?;
    let mut by_page: BTreeMap<usize, Vec<&Stroke>> = BTreeMap::new();
    for stroke in strokes.iter().filter(|s| !s.points.is_empty()) {
        by_page.entry(stroke.page).or_default().push(stroke);
    }
    let bytes = draw(pdfium, original, password, &by_page, flatten)?;
    let (changed, hidden_ink_pages) = check(pdfium, original, &bytes, password, &by_page)?;
    if !changed.is_empty() {
        return Err(SaveError::ChangedOutsideDrawing(changed));
    }
    Ok(Built {
        bytes,
        hidden_ink_pages,
    })
}

/// Draws the strokes into a fresh copy of the document and saves that to bytes.
fn draw(
    pdfium: &Pdfium,
    original: &[u8],
    password: Option<&str>,
    by_page: &BTreeMap<usize, Vec<&Stroke>>,
    flatten: &BTreeSet<usize>,
) -> Result<Vec<u8>, PdfiumError> {
    // A fresh document, never the one on screen: the edits below are permanent.
    let doc = pdfium.load_pdf_from_byte_slice(original, password)?;
    let pages: Vec<usize> = by_page.keys().copied().collect();
    let wraps = wrap_counts(original, password, &pages);
    for ((&index, strokes), wraps) in by_page.iter().zip(wraps) {
        // flatten() and transform_with_clip() reload the page internally but leave stale
        // handles in the PdfPage (using it again is a use-after-free), so every step below
        // drops its PdfPage and the next one fetches the page anew.
        if flatten.contains(&index) {
            pdf::page(&doc, index)?.flatten()?;
        }
        let centre = PageInfo::of(&pdf::page(&doc, index)?)?
            .to_user
            .apply(pos2(0.5, 0.5));
        for _ in 0..wraps {
            if !isolate(pdf::page(&doc, index)?, centre) {
                break;
            }
        }
        let mut page = pdf::page(&doc, index)?;
        // One content stream for all strokes. The default strategy writes a new stream per
        // object; `AutomaticOnDrop` silently writes nothing.
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        for stroke in strokes {
            add_stroke(&doc, &mut page, stroke)?;
        }
        page.regenerate_content()?;
    }
    // A full save into memory. (save_to_file would truncate the target before writing.)
    doc.save_to_bytes()
}

/// Wraps the page's current content in `q <huge clip> … Q`, so graphics state that the content
/// leaves behind (a clip, a dash pattern, …) cannot leak into the strokes appended after it.
/// PDFium does not do that by itself. Returns false when there was nothing to wrap (a page
/// without content). Takes the page by value because it must not be used afterwards.
fn isolate(mut page: PdfPage, centre: Pos2) -> bool {
    let r = ISOLATION_CLIP;
    let clip = PdfRect::new_from_values(centre.y - r, centre.x - r, centre.y + r, centre.x + r);
    page.transform_with_clip(1.0, 0.0, 0.0, 1.0, 0.0, 0.0, clip)
        .is_ok()
}

/// How many times to wrap the original content of each of `pages`: 1 + the larger of its stray
/// `Q` count and its unclosed `q` count. A single wrap is not enough for unbalanced content: a
/// stray `Q` pops the wrapper's `q`, and an unclosed `q` swallows the wrapper's `Q`.
fn wrap_counts(original: &[u8], password: Option<&str>, pages: &[usize]) -> Vec<usize> {
    // lopdf parses untrusted bytes here; should it panic on a broken file, treat that like a
    // file it cannot read instead of crashing the app and losing the drawing.
    std::panic::catch_unwind(|| count_wraps(original, password, pages))
        .unwrap_or_else(|_| vec![None; pages.len()])
        .into_iter()
        // Content lopdf cannot read gets 4 wraps, which covers the usual damage.
        .map(|wraps| wraps.unwrap_or(4).min(MAX_WRAPS))
        .collect()
}

fn count_wraps(original: &[u8], password: Option<&str>, pages: &[usize]) -> Vec<Option<usize>> {
    let options = lopdf::LoadOptions {
        password: password.map(String::from),
        max_decompressed_size: Some(MAX_CONTENT_BYTES),
        ..Default::default()
    };
    let Ok(doc) = lopdf::Document::load_mem_with_options(original, options) else {
        return vec![None; pages.len()];
    };
    let page_ids = doc.get_pages(); // keyed by page number, starting at 1
    let wraps = |index: usize| -> Option<usize> {
        let id = *page_ids.get(&u32::try_from(index + 1).ok()?)?;
        let content = doc
            .get_page_content_with_limit(id, MAX_CONTENT_BYTES)
            .ok()?;
        let operations = lopdf::content::Content::decode(&content).ok()?.operations;
        let (mut depth, mut lowest) = (0i64, 0i64);
        for operation in &operations {
            match operation.operator.as_str() {
                "q" => depth += 1,
                "Q" => {
                    depth -= 1;
                    lowest = lowest.min(depth);
                }
                _ => {}
            }
        }
        let unbalanced = (-lowest).max(depth);
        Some(1 + usize::try_from(unbalanced).unwrap_or(0))
    };
    pages.iter().map(|&index| wraps(index)).collect()
}

/// Adds one stroke to `page` as a path object: stroked with round caps and joins, not filled.
fn add_stroke<'a>(
    doc: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    stroke: &Stroke,
) -> Result<(), PdfiumError> {
    let Some(&start) = stroke.points.first() else {
        return Ok(());
    };
    let pt = PdfPoints::new;
    let [r, g, b, a] = stroke.color.to_srgba_unmultiplied();
    let color = Some(PdfColor::new(r, g, b, a));
    let mut path = PdfPagePathObject::new(
        doc,
        pt(start.x),
        pt(start.y),
        color,
        Some(pt(stroke.width)),
        None,
    )?;
    path.set_line_cap(PdfPageObjectLineCap::Round)?;
    path.set_line_join(PdfPageObjectLineJoin::Round)?;
    for segment in stroke.segments() {
        match segment {
            // new() has already moved to the start point.
            Segment::MoveTo(_) => {}
            // A dot is a zero-length line: round caps make it a disc in every viewer.
            Segment::LineTo(p) => path.line_to(pt(p.x), pt(p.y))?,
            // pdfium-render takes the END point first, then the two control points.
            Segment::CubicTo(c1, c2, end) => {
                path.bezier_to(pt(end.x), pt(end.y), pt(c1.x), pt(c1.y), pt(c2.x), pt(c2.y))?
            }
        }
    }
    page.objects_mut().add_path_object(path)?;
    Ok(())
}

/// Compares every drawn page of `saved` with the original. Returns the pages that changed
/// outside the ink, and the pages where ink is hidden.
fn check(
    pdfium: &Pdfium,
    original: &[u8],
    saved: &[u8],
    password: Option<&str>,
    by_page: &BTreeMap<usize, Vec<&Stroke>>,
) -> Result<(Vec<usize>, Vec<usize>), SaveError> {
    let before = pdfium.load_pdf_from_byte_slice(original, password)?;
    let after = pdfium.load_pdf_from_byte_slice(saved, password)?;
    let (mut changed, mut hidden) = (Vec::new(), Vec::new());
    for (&index, strokes) in by_page {
        let page = check_page(
            &pdf::page(&before, index)?,
            &pdf::page(&after, index)?,
            strokes,
        )?;
        if page.changed_pixels > CHANGED_PIXEL_ALLOWANCE {
            changed.push(index);
        }
        if page.ink_hidden {
            hidden.push(index);
        }
    }
    Ok((changed, hidden))
}

/// What the check of one page found.
struct PageCheck {
    /// Pixels outside the ink that differ from the original.
    changed_pixels: usize,
    /// Some stroke has fewer than half of its centre line visible.
    ink_hidden: bool,
}

/// Renders the page before and after with the same settings and compares the two bitmaps.
fn check_page(
    before: &PdfPage,
    after: &PdfPage,
    strokes: &[&Stroke],
) -> Result<PageCheck, SaveError> {
    let info = PageInfo::of(before)?;
    let Some(from_user) = info.to_user.inverse() else {
        return Err(SaveError::Pdf("a drawn page has no area".into()));
    };
    let thinnest = strokes
        .iter()
        .map(|s| s.width)
        .fold(f32::INFINITY, f32::min);
    // At least 3 px across the thinnest stroke, so the pixels on a centre line are pure ink.
    let scale = (3.0 / thinnest)
        .max(2.0)
        .min(MAX_CHECK_SIDE / info.size.max_elem());
    let config = PdfRenderConfig::new().scale_page_by_factor(scale);
    let old = pdf::render_page(before, &config)?;
    let new = pdf::render_page(after, &config)?;
    if old.size != new.size {
        return Ok(PageCheck {
            changed_pixels: usize::MAX,
            ink_hidden: false,
        });
    }
    let [width, height] = new.size;
    let to_pixels = |p: Pos2| {
        let unit = from_user.apply(p);
        pos2(unit.x * width as f32, unit.y * height as f32)
    };
    // Everything the ink may touch, anti-aliasing included: the comparison skips it.
    let mut ink = vec![false; width * height];
    let mut ink_hidden = false;
    for stroke in strokes {
        let centre_line: Vec<Pos2> = stroke
            .polyline(1.0 / scale)
            .into_iter()
            .map(to_pixels)
            .collect();
        let radius = stroke.width / 2.0 * scale + 2.0;
        let (mut samples, mut visible) = (0, 0);
        for pair in centre_line.windows(2) {
            for p in points_along(pair[0], pair[1], 1.0) {
                stamp_disc(&mut ink, width, height, p, radius);
                if let Some(color) = pixel(&new, p) {
                    samples += 1;
                    if near(color, stroke.color, INK_TOLERANCE) {
                        visible += 1;
                    }
                }
            }
        }
        if visible * 2 < samples {
            ink_hidden = true;
        }
    }
    let changed_pixels = (0..width * height)
        .filter(|&i| !ink[i] && !near(old.pixels[i], new.pixels[i], PIXEL_TOLERANCE))
        .count();
    Ok(PageCheck {
        changed_pixels,
        ink_hidden,
    })
}

/// Marks the pixels whose centre lies within `radius` of `p`.
fn stamp_disc(mask: &mut [bool], width: usize, height: usize, p: Pos2, radius: f32) {
    // Float-to-integer `as` saturates: anything below 0 becomes 0.
    let first = |v: f32| (v - radius).floor() as usize;
    let end = |v: f32, size: usize| ((v + radius).ceil() as usize).min(size);
    for y in first(p.y)..end(p.y, height) {
        for x in first(p.x)..end(p.x, width) {
            if pos2(x as f32 + 0.5, y as f32 + 0.5).distance(p) <= radius {
                mask[y * width + x] = true;
            }
        }
    }
}

/// The colour of the pixel under `p`, if `p` is on the image.
fn pixel(image: &ColorImage, p: Pos2) -> Option<Color32> {
    let [width, height] = image.size;
    let (x, y) = (p.x.floor(), p.y.floor());
    if x < 0.0 || y < 0.0 || x >= width as f32 || y >= height as f32 {
        return None;
    }
    image.pixels.get(y as usize * width + x as usize).copied()
}

/// Whether every colour channel of `a` is within `tolerance` of `b`'s.
fn near(a: Color32, b: Color32, tolerance: u8) -> bool {
    let (a, b) = (a.to_array(), b.to_array());
    (0..3).all(|i| a[i].abs_diff(b[i]) <= tolerance)
}

/// Writes `bytes` to `path` without ever leaving a half-written file there: writes a temporary
/// file in the same folder, flushes it to disk, then renames it over `path`.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
    let temp = path.with_file_name(format!(".{}.pdf-draw.tmp", name.to_string_lossy()));
    let result = write_and_rename(&temp, path, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_and_rename(temp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temp, path)
}

/// The suggested name for the saved copy: `<stem>-signed.pdf` next to the original.
pub fn default_save_path(original: &Path) -> PathBuf {
    let stem = original
        .file_stem()
        .map_or("document".into(), |stem| stem.to_string_lossy());
    original.with_file_name(format!("{stem}-signed.pdf"))
}

/// Whether `a` and `b` name the same file, also when `b` does not exist yet (then its folder is
/// compared, plus its name). Links, `..` and letter case (macOS) are resolved.
pub fn is_same_file(a: &Path, b: &Path) -> bool {
    match (canonical(a), canonical(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn canonical(path: &Path) -> Option<PathBuf> {
    if let Ok(path) = path.canonicalize() {
        return Some(path);
    }
    let folder = match path.parent()? {
        folder if folder.as_os_str().is_empty() => Path::new("."),
        folder => folder,
    };
    Some(folder.canonicalize().ok()?.join(path.file_name()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_disc_marks_pixel_centres_in_reach_and_stays_on_the_image() {
        let mut mask = vec![false; 5 * 4];
        stamp_disc(&mut mask, 5, 4, pos2(0.0, 0.0), 1.6);
        let marked: Vec<usize> = (0..mask.len()).filter(|&i| mask[i]).collect();
        assert_eq!(marked, [0, 1, 5]); // centres (0.5, 0.5), (1.5, 0.5), (0.5, 1.5)
        stamp_disc(&mut mask, 5, 4, pos2(-50.0, 70.0), 3.0); // far off: nothing, no panic
        stamp_disc(&mut mask, 5, 4, pos2(4.9, 3.9), 0.6); // centre (4.5, 3.5) is 0.57 away
        assert!(mask[3 * 5 + 4]);
    }

    /// A fresh, empty folder for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdf-draw-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_save_path_adds_signed() {
        assert_eq!(
            default_save_path(Path::new("/a/b/contract.pdf")),
            Path::new("/a/b/contract-signed.pdf")
        );
        assert_eq!(
            default_save_path(Path::new("March 2026.PDF")),
            Path::new("March 2026-signed.pdf")
        );
    }

    #[test]
    fn same_file_is_recognised_through_other_spellings() {
        let dir = temp_dir("same");
        let original = dir.join("contract.pdf");
        fs::write(&original, b"x").unwrap();
        assert!(is_same_file(&original, &original));
        assert!(!is_same_file(&original, &dir.join("contract-signed.pdf")));
        // A target that does not exist yet is compared by folder and name.
        assert!(!is_same_file(&original, &dir.join("new.pdf")));
        fs::create_dir(dir.join("sub")).unwrap();
        assert!(is_same_file(&original, &dir.join("sub/../contract.pdf")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn write_atomically_replaces_the_target_and_leaves_no_temp_file() {
        let dir = temp_dir("atomic");
        let target = dir.join("out.pdf");
        write_atomically(&target, b"first").unwrap();
        write_atomically(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["out.pdf"]);
        // A folder that does not exist: an error, nothing left behind.
        assert!(write_atomically(&dir.join("missing/out.pdf"), b"x").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn near_compares_each_channel() {
        let ink = Color32::from_rgb(198, 40, 40);
        assert!(near(Color32::from_rgb(210, 30, 52), ink, 12));
        assert!(!near(Color32::from_rgb(211, 40, 40), ink, 12));
        assert!(!near(Color32::WHITE, ink, 40));
    }
}
