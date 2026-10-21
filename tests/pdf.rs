//! End-to-end tests of opening and saving with the vendored PDFium: strokes are saved into a
//! copy, the copy is opened again, and its pixels are checked.

mod common;

use common::*;
use eframe::egui::{Pos2, pos2};
use pdf_draw::ink::Stroke;
use pdf_draw::pdf::{Document, OpenError};
use pdf_draw::save::{self, SaveError};
use std::collections::BTreeSet;

/// Points on the page (fractions of the displayed page, y down) that test strokes go through:
/// left and right half (several fixtures clip to the left half), middle, and near a corner.
const TARGETS: [Pos2; 4] = [
    pos2(0.25, 0.3),
    pos2(0.7, 0.6),
    pos2(0.5, 0.5),
    pos2(0.9, 0.08),
];

fn strokes_through(doc: &Document, page: usize, targets: &[Pos2]) -> Vec<Stroke> {
    targets
        .iter()
        .map(|&t| scribble(doc, page, t, 4.0))
        .collect()
}

fn build(doc: &Document, strokes: &[Stroke]) -> Result<save::Built, SaveError> {
    save::build(doc.bytes(), doc.password(), strokes, &BTreeSet::new())
}

/// Draws through `TARGETS` on every page of the fixture, saves, opens the copy again, and checks
/// that the ink is where it was drawn and nothing else on the page changed.
fn round_trip(name: &str) {
    let original = fixture(name);
    let doc = open(original.clone());
    let strokes: Vec<Stroke> = (0..doc.pages.len())
        .flat_map(|page| strokes_through(&doc, page, &TARGETS))
        .collect();
    let built = build(&doc, &strokes).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        built.hidden_ink_pages,
        [] as [usize; 0],
        "{name}: ink hidden"
    );

    let saved = open(built.bytes);
    assert_eq!(saved.pages.len(), doc.pages.len(), "{name}: page count");
    for page in 0..doc.pages.len() {
        assert_eq!(
            saved.pages[page].size, doc.pages[page].size,
            "{name}: page size"
        );
        let before = render(&doc, page);
        let after = render(&saved, page);
        for target in TARGETS {
            assert!(
                has_color_near(&after, target, INK, 2),
                "{name} page {page}: no ink at {target:?}"
            );
            assert!(
                !has_color_near(&before, target, INK, 2),
                "{name}: ink already there before saving"
            );
        }
        // The scribbles span about ±15 pt (30 px at 2×) around each target.
        let changed = changed_pixels(&before, &after, &TARGETS, 40.0);
        assert_eq!(
            changed, 0,
            "{name} page {page}: {changed} pixels changed off the ink"
        );
    }
    assert_eq!(doc.bytes(), original, "{name}: the original bytes changed");
}

#[test]
fn plain_page() {
    round_trip("letter");
}

#[test]
fn rotated_pages() {
    for name in ["rotate90", "rotate180", "rotate270", "inherited-rotate"] {
        round_trip(name);
    }
}

#[test]
fn cropped_and_offset_pages() {
    for name in [
        "cropbox",
        "offset-mediabox",
        "negative-mediabox",
        "rot90-crop",
        "inherited-crop",
    ] {
        round_trip(name);
    }
}

/// Content that leaves a transform, dash pattern or clip behind, or has unbalanced q/Q, must not
/// move, dash or clip away the strokes drawn after it.
#[test]
fn content_with_leftover_graphics_state() {
    for name in ["dirty-state", "dirty-clip", "unclosed-q", "stray-Q"] {
        round_trip(name);
    }
}

#[test]
fn only_drawn_pages_change() {
    let doc = open(fixture("multipage"));
    let strokes = strokes_through(&doc, 1, &TARGETS);
    let saved = open(build(&doc, &strokes).unwrap().bytes);
    for page in [0, 2] {
        let changed = changed_pixels(&render(&doc, page), &render(&saved, page), &[], 0.0);
        assert_eq!(changed, 0, "page {page} changed");
    }
    assert!(has_color_near(&render(&saved, 1), TARGETS[0], INK, 2));
}

/// The fixture's form field is at x 300..540, y 380..410 pt (from the bottom) and paints itself
/// opaque, so ink under it is hidden until the field is flattened into the page.
#[test]
fn ink_under_a_form_field() {
    let doc = open(fixture("annotated"));
    let in_field = pos2(420.0 / 612.0, (792.0 - 395.0) / 792.0);
    let strokes = strokes_through(&doc, 0, &[in_field, TARGETS[0]]);

    let built = build(&doc, &strokes).unwrap();
    assert_eq!(built.hidden_ink_pages, [0]);
    assert!(!has_color_near(
        &render(&open(built.bytes), 0),
        in_field,
        INK,
        2
    ));

    let flattened = save::build(doc.bytes(), None, &strokes, &BTreeSet::from([0])).unwrap();
    assert_eq!(flattened.hidden_ink_pages, [] as [usize; 0]);
    let saved = open(flattened.bytes);
    assert!(has_color_near(&render(&saved, 0), in_field, INK, 2));
    assert!(has_color_near(&render(&saved, 0), TARGETS[0], INK, 2));
}

#[test]
fn ink_beside_a_form_field_keeps_the_field() {
    let doc = open(fixture("annotated"));
    let built = build(&doc, &strokes_through(&doc, 0, &[TARGETS[0]])).unwrap();
    assert_eq!(built.hidden_ink_pages, [] as [usize; 0]);
    let text = String::from_utf8_lossy(&built.bytes);
    assert!(text.contains("signer_name"), "the form field is gone");
}

/// PDFium drops a page font that only a resource-less form XObject uses. The check must catch
/// that instead of saving a page with missing text.
#[test]
fn damage_outside_the_drawing_is_refused() {
    let doc = open(fixture("xobject-no-res"));
    let result = build(&doc, &strokes_through(&doc, 0, &[TARGETS[0]]));
    match result {
        Err(SaveError::ChangedOutsideDrawing(pages)) => assert_eq!(pages, [0]),
        other => panic!("expected ChangedOutsideDrawing, got {other:?}"),
    }
}

#[test]
fn owner_password_pdf_saves_and_stays_encrypted() {
    // Encrypted with an owner password only, so it opens without one.
    let doc = open(fixture("encrypted"));
    assert_eq!(doc.password(), None);
    let built = build(&doc, &strokes_through(&doc, 0, &TARGETS)).unwrap();
    assert!(
        built.bytes.windows(8).any(|w| w == b"/Encrypt"),
        "the copy lost its encryption"
    );
    let saved = open(built.bytes);
    assert!(has_color_near(&render(&saved, 0), TARGETS[1], INK, 2));
}

#[test]
fn digital_signatures_are_counted() {
    assert_eq!(open(fixture("signed")).signature_count, 1);
    assert_eq!(open(fixture("letter")).signature_count, 0);
}

#[test]
fn a_dot_is_saved() {
    let doc = open(fixture("letter"));
    let mut dot = Stroke::new(0, INK, 4.0);
    dot.push(user(&doc, 0, TARGETS[2]), 0.0);
    let saved = open(build(&doc, &[dot]).unwrap().bytes);
    assert!(has_color_near(&render(&saved, 0), TARGETS[2], INK, 1));
}

#[test]
fn bad_files_and_passwords_are_errors() {
    assert!(matches!(
        Document::open(b"not a pdf".to_vec(), None),
        Err(OpenError::Invalid(_))
    ));
    assert!(matches!(
        Document::open(Vec::new(), None),
        Err(OpenError::Invalid(_))
    ));
    // A NUL in a password would poison PDFium for the whole process: refused up front.
    assert!(matches!(
        Document::open(fixture("letter"), Some("a\0b")),
        Err(OpenError::WrongPassword)
    ));
    // PDFium still works afterwards.
    open(fixture("letter"));
}

#[test]
fn page_geometry_follows_rotation_and_crop() {
    // Rotated by 90°, the displayed page is landscape; its top-left corner is the page's
    // bottom-left in user space.
    let doc = open(fixture("rotate90"));
    assert_eq!(doc.pages[0].size, eframe::egui::vec2(792.0, 612.0));
    let corner = user(&doc, 0, pos2(0.0, 0.0));
    assert!(corner.distance(pos2(0.0, 0.0)) < 1e-3, "{corner:?}");
    // Cropped: the displayed page is the CropBox [50 100 562 742].
    let doc = open(fixture("cropbox"));
    assert_eq!(doc.pages[0].size, eframe::egui::vec2(512.0, 642.0));
    let top_left = user(&doc, 0, pos2(0.0, 0.0));
    assert!(top_left.distance(pos2(50.0, 742.0)) < 1e-3, "{top_left:?}");
}
