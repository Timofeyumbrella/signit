//! PDFium access for the viewer: binding the library, opening a document, page geometry and
//! rendering. The write path is in `save.rs`.

use crate::geometry::Affine;
use eframe::egui::{ColorImage, Pos2, Vec2, pos2, vec2};
use pdfium_render::prelude::*;
use std::fmt;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The PDFium library, bound on first use.
///
/// PDFium can be bound only once per process (a second bind fails), so the outcome, error
/// included, is kept for the life of the process. After an error, call no other PDFium API:
/// several of them hang or panic when no library is bound.
pub fn pdfium() -> Result<&'static Pdfium, String> {
    static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();
    PDFIUM.get_or_init(bind).as_ref().map_err(String::clone)
}

/// Looks for libpdfium next to the executable, then in `vendor/` of this source tree, then in the
/// system library paths. (`Pdfium::default()` looks in the current directory instead, and panics
/// when the library is not there.)
fn bind() -> Result<Pdfium, String> {
    let next_to_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from));
    let vendored = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/pdfium/lib"));
    let mut tried = Vec::new();
    for dir in next_to_exe.into_iter().chain([vendored]) {
        let path = Pdfium::pdfium_platform_library_name_at_path(&dir);
        match Pdfium::bind_to_library(&path) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(_) => tried.push(path.display().to_string()),
        }
    }
    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Ok(Pdfium::new(bindings)),
        Err(e) => Err(format!(
            "tried {} and the system library paths ({e:?})",
            tried.join(", ")
        )),
    }
}

/// Size and coordinate mapping of one page as it is displayed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageInfo {
    /// Displayed size in pt: the visible box (CropBox ∩ MediaBox), turned by /Rotate.
    pub size: Vec2,
    /// Maps the unit square over the displayed page (x right, y down, (0, 0) at the top-left
    /// corner) to PDF user space.
    pub to_user: Affine,
}

impl PageInfo {
    /// Reads the geometry of `page`.
    pub(crate) fn of(page: &PdfPage) -> Result<PageInfo, PdfiumError> {
        // Ask PDFium where three corners of a bitmap of known size land on the page. Building
        // the mapping from page.boundaries() instead fails, or is silently wrong, when the
        // MediaBox or CropBox is inherited from the page tree.
        let config = PdfRenderConfig::new().set_fixed_size(1000, 1000);
        let corner = |x, y| -> Result<Pos2, PdfiumError> {
            let (ux, uy) = page.pixels_to_points(x, y, &config)?;
            Ok(pos2(ux.value, uy.value))
        };
        Ok(PageInfo {
            size: vec2(page.width().value, page.height().value),
            to_user: Affine::from_points(corner(0, 0)?, corner(1000, 0)?, corner(0, 1000)?),
        })
    }
}

/// Why a PDF could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The PDF is encrypted and needs a password.
    PasswordRequired,
    /// The password is neither the PDF's user password nor its owner password.
    WrongPassword,
    /// PDFium cannot read the file.
    Invalid(String),
    /// The PDFium library is not available.
    Library(String),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::PasswordRequired => write!(f, "This PDF is protected by a password."),
            OpenError::WrongPassword => write!(f, "The password is not correct."),
            OpenError::Invalid(why) => write!(f, "This file cannot be opened as a PDF: {why}"),
            OpenError::Library(why) => write!(f, "PDFium library not found: {why}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// An open PDF, with everything the viewer needs to know about it at hand.
pub struct Document {
    doc: PdfDocument<'static>,
    bytes: Vec<u8>,
    password: Option<String>,
    /// Geometry of every page, in order.
    pub pages: Vec<PageInfo>,
    /// Number of digital signatures. Saving a copy invalidates them.
    pub signature_count: usize,
}

impl Document {
    /// Opens a PDF from the bytes of the file. Pass `None` first; when that fails with
    /// [`OpenError::PasswordRequired`], ask for a password and try again with it.
    pub fn open(bytes: Vec<u8>, password: Option<&str>) -> Result<Document, OpenError> {
        // pdfium-render panics on a NUL in the password while it holds PDFium's global lock,
        // which then refuses to work for the rest of the process.
        if password.is_some_and(|p| p.contains('\0')) {
            return Err(OpenError::WrongPassword);
        }
        let pdfium = pdfium().map_err(OpenError::Library)?;
        // PDFium keeps its own copy of the bytes; ours is for building the saved file later.
        let doc = match pdfium.load_pdf_from_byte_vec(bytes.clone(), password) {
            Ok(doc) => doc,
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                return Err(if password.is_none() {
                    OpenError::PasswordRequired
                } else {
                    OpenError::WrongPassword
                });
            }
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError)) => {
                return Err(OpenError::Invalid(
                    "it is not a PDF, or it is damaged".into(),
                ));
            }
            Err(e) => return Err(OpenError::Invalid(format!("{e:?}"))),
        };
        let mut pages = Vec::new();
        for index in doc.pages().as_range() {
            let info = doc
                .pages()
                .get(index)
                .and_then(|page| PageInfo::of(&page))
                .map_err(|e| OpenError::Invalid(format!("page {}: {e:?}", index + 1)))?;
            if info.to_user.inverse().is_none() {
                let why = format!("page {} has no area", index + 1);
                return Err(OpenError::Invalid(why));
            }
            pages.push(info);
        }
        if pages.is_empty() {
            return Err(OpenError::Invalid("it has no pages".into()));
        }
        let signature_count = usize::from(doc.signatures().len());
        Ok(Document {
            doc,
            bytes,
            password: password.map(String::from),
            pages,
            signature_count,
        })
    }

    /// The bytes of the opened file.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The password the document was opened with.
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// Renders page `index` into an image of exactly `[width, height]` pixels, form fields
    /// included. Use `view::render_size` for the size: it keeps the page's aspect ratio.
    pub fn render(&self, index: usize, [width, height]: [usize; 2]) -> anyhow::Result<ColorImage> {
        let config =
            PdfRenderConfig::new().set_fixed_size(i32::try_from(width)?, i32::try_from(height)?);
        Ok(render_page(&page(&self.doc, index)?, &config)?)
    }
}

/// Page `index` (0-based) of `doc`.
pub(crate) fn page<'a>(doc: &PdfDocument<'a>, index: usize) -> Result<PdfPage<'a>, PdfiumError> {
    let index = PdfPageIndex::try_from(index).map_err(|_| PdfiumError::PageIndexOutOfBounds)?;
    doc.pages().get(index)
}

/// Renders `page` into an egui image. Keep transforms and clips out of `config`: either one
/// silently turns off the drawing of form fields.
pub(crate) fn render_page(
    page: &PdfPage,
    config: &PdfRenderConfig,
) -> Result<ColorImage, PdfiumError> {
    let bitmap = page.render_with_config(config)?;
    let size = [bitmap.width() as usize, bitmap.height() as usize];
    // PDFium's BGRA bitmaps have no row padding and, with pdfium-render's default byte-order
    // flag, hold R, G, B, A with straight alpha: exactly what this constructor expects.
    Ok(ColorImage::from_rgba_unmultiplied(
        size,
        &bitmap.as_rgba_bytes(),
    ))
}
