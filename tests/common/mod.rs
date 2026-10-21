//! Test PDFs, generated in memory with lopdf, and helpers to draw on them and look at the result.
//!
//! Most fixtures recreate a way real PDFs differ from the simple case (rotated pages, offset or
//! inherited page boxes, content that leaves the graphics state dirty, …); each one broke a naive
//! way of saving strokes during the research for this app.

#![allow(dead_code)] // each test binary uses a different part of this module

use eframe::egui::{Color32, ColorImage, Pos2, Vec2, pos2, vec2};
use lopdf::{Dictionary, Document as LoDocument, Object, ObjectId, Stream, dictionary};
use pdf_draw::ink::Stroke;
use pdf_draw::pdf::Document;
use std::path::PathBuf;

pub const LETTER: [f32; 4] = [0.0, 0.0, 612.0, 792.0];
/// The ink colour of test strokes: nothing else in the fixtures is this red.
pub const INK: Color32 = Color32::from_rgb(220, 0, 0);

fn real(v: f32) -> Object {
    Object::Real(v)
}

fn rect(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|&v| real(v)).collect())
}

/// A one-page "contract" for a `w` × `h` page whose MediaBox starts at (0, 0).
pub fn contract(w: f32, h: f32, title: &str) -> String {
    format!(
        "0 0 0 RG 0.75 w 36 36 {bw} {bh} re S\n\
         BT /F1 16 Tf 72 {t1} Td ({title}) Tj ET\n\
         BT /F1 11 Tf 72 {t2} Td 15 TL (This agreement is made between Acme Corp and the Contractor.) Tj \
         T* (The Contractor will provide the services described in Schedule A.) Tj \
         T* (Payment is due within thirty days of each invoice.) Tj \
         T* (Either party may terminate with fourteen days written notice.) Tj ET\n\
         BT /F1 11 Tf 72 150 Td (Signed: ______________________     Date: ______________) Tj ET\n\
         BT /F1 9 Tf 72 54 Td (Footer: {title} - ref 2026-10) Tj ET\n",
        bw = w - 72.0,
        bh = h - 72.0,
        t1 = h - 66.0,
        t2 = h - 96.0,
    )
}

/// The same content for a MediaBox whose lower-left corner is (`ox`, `oy`).
fn shifted(content: &str, ox: f32, oy: f32) -> String {
    format!("q 1 0 0 1 {ox} {oy} cm\n{content}Q\n")
}

struct Builder {
    doc: LoDocument,
    pages_id: ObjectId,
    font_id: ObjectId,
    kids: Vec<ObjectId>,
}

impl Builder {
    fn new() -> Self {
        let mut doc = LoDocument::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        Builder {
            doc,
            pages_id,
            font_id,
            kids: Vec::new(),
        }
    }

    fn fonts(&self) -> Dictionary {
        dictionary! { "Font" => dictionary! { "F1" => self.font_id } }
    }

    /// Adds a page; `dict` holds everything except Type, Parent and Contents.
    fn page(&mut self, mut dict: Dictionary, content: &str) -> ObjectId {
        let contents = self
            .doc
            .add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        dict.set("Type", "Page");
        dict.set("Parent", self.pages_id);
        dict.set("Contents", contents);
        let id = self.doc.add_object(dict);
        self.kids.push(id);
        id
    }

    fn page_with(&mut self, media: [f32; 4], extra: Dictionary, content: &str) -> ObjectId {
        let mut dict = dictionary! { "MediaBox" => rect(media), "Resources" => self.fonts() };
        for (k, v) in extra.iter() {
            dict.set(k.clone(), v.clone());
        }
        self.page(dict, content)
    }

    fn finish(mut self, pages_extra: Dictionary, catalog_extra: Dictionary) -> Vec<u8> {
        let mut pages = dictionary! {
            "Type" => "Pages",
            "Kids" => self.kids.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>(),
            "Count" => self.kids.len() as i64,
        };
        for (k, v) in pages_extra.iter() {
            pages.set(k.clone(), v.clone());
        }
        self.doc
            .objects
            .insert(self.pages_id, Object::Dictionary(pages));
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => self.pages_id };
        for (k, v) in catalog_extra.iter() {
            catalog.set(k.clone(), v.clone());
        }
        let catalog_id = self.doc.add_object(catalog);
        self.doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        self.doc
            .save_to(&mut bytes)
            .expect("lopdf writes the fixture");
        bytes
    }
}

fn single(media: [f32; 4], extra: Dictionary, content: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.page_with(media, extra, content);
    b.finish(dictionary! {}, dictionary! {})
}

fn letter() -> String {
    contract(612.0, 792.0, "SERVICE AGREEMENT")
}

/// The fixture called `name` (see the match arms), as PDF bytes.
pub fn fixture(name: &str) -> Vec<u8> {
    let letter = letter();
    match name {
        "letter" => single(LETTER, dictionary! {}, &letter),
        "rotate90" | "rotate180" | "rotate270" => {
            let degrees: i64 = name["rotate".len()..].parse().unwrap();
            single(LETTER, dictionary! { "Rotate" => degrees }, &letter)
        }
        // /Rotate, /MediaBox and /Resources all inherited from the /Pages node.
        "inherited-rotate" => {
            let mut b = Builder::new();
            let fonts = b.fonts();
            b.page(dictionary! {}, &letter);
            let pages =
                dictionary! { "Rotate" => 90, "MediaBox" => rect(LETTER), "Resources" => fonts };
            b.finish(pages, dictionary! {})
        }
        "cropbox" => single(
            LETTER,
            dictionary! { "CropBox" => rect([50.0, 100.0, 562.0, 742.0]) },
            &letter,
        ),
        "offset-mediabox" => single(
            [100.0, 200.0, 712.0, 992.0],
            dictionary! {},
            &shifted(&letter, 100.0, 200.0),
        ),
        "negative-mediabox" => single(
            [-306.0, -396.0, 306.0, 396.0],
            dictionary! {},
            &shifted(&letter, -306.0, -396.0),
        ),
        "rot90-crop" => single(
            LETTER,
            dictionary! { "CropBox" => rect([40.0, 60.0, 500.0, 700.0]), "Rotate" => 90 },
            &letter,
        ),
        // CropBox and /Rotate only on the /Pages node, where page.boundaries() cannot see them.
        "inherited-crop" => {
            let mut b = Builder::new();
            b.page_with(LETTER, dictionary! {}, &letter);
            let pages =
                dictionary! { "CropBox" => rect([40.0, 60.0, 500.0, 700.0]), "Rotate" => 90 };
            b.finish(pages, dictionary! {})
        }
        // Graphics state left modified at the end of the content, without q/Q.
        "dirty-state" => {
            let mut b = Builder::new();
            let mut res = b.fonts();
            res.set(
                "ExtGState",
                dictionary! { "GSa" => dictionary! { "Type" => "ExtGState", "CA" => real(0.4), "ca" => real(0.4) } },
            );
            let content = format!(
                "{letter}2 0 0 2 30 40 cm\n0 0 1 RG 5 w 0.3 g\n[4 6] 0 d\n/GSa gs\n10 20 60 40 re B\n"
            );
            b.page(
                dictionary! { "MediaBox" => rect(LETTER), "Resources" => res },
                &content,
            );
            b.finish(dictionary! {}, dictionary! {})
        }
        // A clip to the left half left active at the end of the content.
        "dirty-clip" => single(
            LETTER,
            dictionary! {},
            &format!("{letter}0 0 306 792 re W n\n0.85 0.9 1 rg 40 300 230 40 re f\n"),
        ),
        // Dash and clip, then a q that is never closed: one wrapping Q only pops that q.
        "unclosed-q" => single(
            LETTER,
            dictionary! {},
            &format!("{letter}[4 6] 0 d 0 0 306 792 re W n\nq 0.85 0.9 1 rg 40 300 230 40 re f\n"),
        ),
        // A stray Q pops a wrapping q, so what follows runs outside a single wrap.
        "stray-Q" => single(
            LETTER,
            dictionary! {},
            &format!("{letter}Q\n[4 6] 0 d 0 0 306 792 re W n\n0.85 0.9 1 rg 40 300 230 40 re f\n"),
        ),
        "multipage" => {
            let mut b = Builder::new();
            b.page_with(
                LETTER,
                dictionary! {},
                &contract(612.0, 792.0, "PAGE 1 - US LETTER"),
            );
            b.page_with(
                [0.0, 0.0, 595.276, 841.89],
                dictionary! {},
                &contract(595.276, 841.89, "PAGE 2 - A4"),
            );
            b.page_with(
                [0.0, 0.0, 792.0, 612.0],
                dictionary! {},
                &contract(792.0, 612.0, "PAGE 3 - LANDSCAPE"),
            );
            b.finish(dictionary! {}, dictionary! {})
        }
        // A form text field whose appearance paints its rect [300 380 540 410] opaque yellow.
        "annotated" => {
            let mut b = Builder::new();
            let helv = dictionary! { "Font" => dictionary! { "Helv" => b.font_id } };
            let ap = b.doc.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Form",
                    "BBox" => rect([0.0, 0.0, 240.0, 30.0]), "Resources" => helv.clone(),
                },
                b"1 1 0.8 rg 0 0 240 30 re f 0.4 g 0.5 w 0.25 0.25 239.5 29.5 re S \
                  /Tx BMC q BT 0 g /Helv 14 Tf 6 9 Td (Jane Q. Public) Tj ET Q EMC"
                    .to_vec(),
            ));
            let widget = b.doc.new_object_id();
            let page = b.page_with(
                LETTER,
                dictionary! { "Annots" => vec![Object::Reference(widget)] },
                &letter,
            );
            b.doc.objects.insert(
                widget,
                Object::Dictionary(dictionary! {
                    "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
                    "T" => Object::string_literal("signer_name"),
                    "V" => Object::string_literal("Jane Q. Public"),
                    "Rect" => rect([300.0, 380.0, 540.0, 410.0]), "F" => 4, "P" => page,
                    "DA" => Object::string_literal("/Helv 14 Tf 0 g"),
                    "AP" => dictionary! { "N" => ap },
                }),
            );
            let acroform = dictionary! {
                "Fields" => vec![Object::Reference(widget)],
                "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
                "DR" => helv,
            };
            b.finish(dictionary! {}, dictionary! { "AcroForm" => acroform })
        }
        // A form XObject without /Resources whose font F2 lives only in the page's resources.
        // PDFium drops F2 from the page when it rewrites the page content.
        "xobject-no-res" => {
            let mut b = Builder::new();
            let times = b.doc.add_object(dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman", "Encoding" => "WinAnsiEncoding",
            });
            let fx = b.doc.add_object(Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => rect([0.0, 0.0, 260.0, 60.0]) },
                b"0.6 0 0.6 RG 1 w 1 1 258 58 re S BT /F2 14 Tf 10 24 Td (Times text without Resources) Tj ET"
                    .to_vec(),
            ));
            let res = dictionary! {
                "Font" => dictionary! { "F1" => b.font_id, "F2" => times },
                "XObject" => dictionary! { "Fx1" => fx },
            };
            let content = format!("{letter}q 1 0 0 1 300 380 cm /Fx1 Do Q\n");
            b.page(
                dictionary! { "MediaBox" => rect(LETTER), "Resources" => res },
                &content,
            );
            b.finish(dictionary! {}, dictionary! {})
        }
        // Made with external tools, so they are committed (see tests/fixtures/README.md).
        "encrypted" | "signed" => std::fs::read(fixture_file(name)).unwrap(),
        _ => panic!("unknown fixture {name}"),
    }
}

pub fn fixture_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}.pdf"))
}

/// Opens fixture bytes, panicking with the reason on failure.
pub fn open(bytes: Vec<u8>) -> Document {
    Document::open(bytes, None).unwrap_or_else(|e| panic!("cannot open: {e}"))
}

/// Converts a point given as fractions of the displayed page (0..1, y down) to PDF user space.
pub fn user(doc: &Document, page: usize, fraction: Pos2) -> Pos2 {
    doc.pages[page].to_user.apply(fraction)
}

/// A short scribble on `page` whose path passes exactly through `target` (fractions of the
/// displayed page, y down). It is built in displayed-page points, so it looks the same on any
/// rotated or cropped page.
pub fn scribble(doc: &Document, page: usize, target: Pos2, width: f32) -> Stroke {
    let size = doc.pages[page].size;
    let centre = vec2(target.x * size.x, target.y * size.y);
    let offsets = [
        vec2(-14.0, -5.0),
        vec2(-6.0, 4.0),
        Vec2::ZERO,
        vec2(7.0, -6.0),
        vec2(14.0, 3.0),
    ];
    let mut stroke = Stroke::new(page, INK, width);
    for offset in offsets {
        let point = centre + offset;
        let fraction = pos2(point.x / size.x, point.y / size.y);
        stroke.push(user(doc, page, fraction), 0.0);
    }
    stroke
}

/// Renders `page` of `doc` at 2 pixels per pt of its displayed size.
pub fn render(doc: &Document, page: usize) -> ColorImage {
    let size = doc.pages[page].size * 2.0;
    doc.render(page, [size.x.round() as usize, size.y.round() as usize])
        .unwrap()
}

/// Whether a pixel within `reach` px of the point at `fraction` of the image has colour `color`.
pub fn has_color_near(image: &ColorImage, fraction: Pos2, color: Color32, reach: i64) -> bool {
    let [w, h] = image.size;
    let (cx, cy) = (
        (fraction.x * w as f32) as i64,
        (fraction.y * h as f32) as i64,
    );
    (cy - reach..=cy + reach).any(|y| {
        (cx - reach..=cx + reach).any(|x| {
            (0..w as i64).contains(&x)
                && (0..h as i64).contains(&y)
                && close(image.pixels[y as usize * w + x as usize], color, 40)
        })
    })
}

pub fn close(a: Color32, b: Color32, tolerance: u8) -> bool {
    let (a, b) = (a.to_array(), b.to_array());
    (0..3).all(|i| a[i].abs_diff(b[i]) <= tolerance)
}

/// How many pixels differ between two same-sized images, ignoring a square of `skip` px around
/// each of the `fractions`.
pub fn changed_pixels(a: &ColorImage, b: &ColorImage, fractions: &[Pos2], skip: f32) -> usize {
    assert_eq!(a.size, b.size, "the page size changed");
    let [w, h] = a.size;
    let skipped = |x: usize, y: usize| {
        fractions.iter().any(|f| {
            (x as f32 - f.x * w as f32).abs() <= skip && (y as f32 - f.y * h as f32).abs() <= skip
        })
    };
    (0..w * h)
        .filter(|&i| !close(a.pixels[i], b.pixels[i], 16) && !skipped(i % w, i / w))
        .count()
}
