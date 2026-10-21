//! The egui application: toolbar, page view, pen and eraser input, dialogs, opening and saving.

use crate::history::Drawing;
use crate::ink::{Stroke, points_along};
use crate::pdf::{self, Document, OpenError};
use crate::save::{self, Built};
use crate::view;
use eframe::egui::scroll_area::{DragScroll, ScrollSource};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Event, FontId, Id, Key, KeyboardShortcut, Layout,
    Modifiers, PointerButton, Pos2, Rect, Response, RichText, Sense, Shape, TextureHandle,
    TextureOptions, Ui, Vec2, ViewportCommand, vec2,
};
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals);
// On a US keyboard Cmd+Shift+= arrives as Plus.
const ZOOM_IN_PLUS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus);
const ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);
const FIT_WIDTH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0);

/// The pen colours in the toolbar. Blue is the default: it tells a signature apart from print.
pub const INKS: [(&str, Color32); 3] = [
    ("Blue", Color32::from_rgb(0x1F, 0x3D, 0x99)),
    ("Black", Color32::BLACK),
    ("Red", Color32::from_rgb(0xC6, 0x28, 0x28)),
];
/// Radius of the eraser, in screen points.
const ERASER_RADIUS: f32 = 8.0;
/// Seconds without zooming before pages are rendered again at the new size.
const ZOOM_SETTLE: f64 = 0.15;
const ZOOM_STEP: f32 = 1.25;
const BACKGROUND: Color32 = Color32::from_gray(0x52);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Eraser,
}

/// Asks where to save, starting from the suggested path.
pub type SaveDialog = Box<dyn FnMut(&Path) -> Option<PathBuf>>;

/// The native file dialogs. Tests replace them, because rfd only works on the main thread.
pub struct Dialogs {
    /// Asks for a PDF to open.
    pub open: Box<dyn FnMut() -> Option<PathBuf>>,
    pub save: SaveDialog,
}

impl Default for Dialogs {
    fn default() -> Self {
        Dialogs {
            open: Box::new(|| {
                rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .pick_file()
            }),
            save: Box::new(|suggested| {
                let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
                if let Some(folder) = suggested.parent() {
                    dialog = dialog.set_directory(folder);
                }
                if let Some(name) = suggested.file_name() {
                    dialog = dialog.set_file_name(name.to_string_lossy());
                }
                dialog.save_file()
            }),
        }
    }
}

/// The open PDF and where it came from.
struct Open {
    path: PathBuf,
    document: Document,
}

/// A rendered page and the bitmap size it was rendered at.
struct PageTexture {
    handle: TextureHandle,
    size: [usize; 2],
}

/// The dialog on top of the window, if any.
enum Dialog {
    Error(String),
    Password {
        path: PathBuf,
        bytes: Vec<u8>,
        input: String,
        wrong: bool,
    },
    /// "Discard your drawing?", then do `Then`.
    Discard(Then),
    /// The PDF carries digital signatures that the saved copy will not.
    Signed,
    /// The copy is built, but some ink would be hidden behind form fields.
    HiddenInk {
        target: PathBuf,
        built: Built,
    },
}

/// What to do once unsaved strokes may be dropped.
enum Then {
    OpenDialog,
    Open(PathBuf),
    Close,
}

/// A button clicked in a dialog.
enum Choice {
    Password,
    Discard,
    SaveSigned,
    Flatten,
    SaveAnyway,
}

pub struct App {
    open: Option<Open>,
    drawing: Drawing,
    /// The stroke being drawn.
    stroke: Option<Stroke>,
    /// While the eraser is dragged: its page, and where it was last in PDF user space.
    eraser_at: Option<(usize, Pos2)>,
    tool: Tool,
    color: Color32,
    /// Pen width in pt.
    width: f32,
    zoom: f32,
    /// Fit the page width on the next frame (after opening, or Cmd+0).
    fit_pending: bool,
    /// Scroll back to the top on the next frame (after opening).
    scroll_reset: bool,
    /// A zoom chosen with the keyboard or toolbar, applied around the view's centre.
    zoom_request: Option<f32>,
    /// `time` of the last zoom step; pages are re-rendered once zooming has settled.
    last_zoom_time: f64,
    /// The scroll offset the last frame was drawn with, and the scroll area's screen rect.
    drawn_offset: Vec2,
    inner: Option<Rect>,
    /// Screen rects of the pages in the last frame.
    page_rects: Vec<Rect>,
    current_page: usize,
    textures: HashMap<usize, PageTexture>,
    failed_pages: BTreeSet<usize>,
    dialog: Option<Dialog>,
    status: String,
    /// The user agreed to discard unsaved strokes and quit.
    allow_close: bool,
    pdfium_error: Option<String>,
    /// The window title last sent.
    title: String,
    pub dialogs: Dialogs,
}

impl Default for App {
    fn default() -> Self {
        App::new()
    }
}

impl App {
    /// An app with no document open. Binds PDFium; if that fails the window explains how to
    /// get the library.
    pub fn new() -> App {
        App {
            open: None,
            drawing: Drawing::default(),
            stroke: None,
            eraser_at: None,
            tool: Tool::Pen,
            color: INKS[0].1,
            width: 1.5,
            zoom: 1.0,
            fit_pending: false,
            scroll_reset: false,
            zoom_request: None,
            last_zoom_time: f64::NEG_INFINITY,
            drawn_offset: Vec2::ZERO,
            inner: None,
            page_rects: Vec::new(),
            current_page: 0,
            textures: HashMap::new(),
            failed_pages: BTreeSet::new(),
            dialog: None,
            status: String::new(),
            allow_close: false,
            pdfium_error: pdf::pdfium().err(),
            title: String::new(),
            dialogs: Dialogs::default(),
        }
    }

    /// Opens the PDF at `path`, replacing the open one (and its strokes) without asking.
    pub fn open_path(&mut self, path: &Path) {
        match std::fs::read(path) {
            Ok(bytes) => self.open_bytes(path.to_path_buf(), bytes, None),
            Err(e) => {
                let message = format!("{} cannot be read: {e}", path.display());
                self.dialog = Some(Dialog::Error(message));
            }
        }
    }

    fn open_bytes(&mut self, path: PathBuf, bytes: Vec<u8>, password: Option<&str>) {
        // Document::open takes the bytes; keep a copy in case it asks for a password.
        match Document::open(bytes.clone(), password) {
            Ok(document) => {
                let pages = document.pages.len();
                self.status = format!("Opened {} ({pages} pages)", file_name(&path));
                self.open = Some(Open { path, document });
                self.drawing = Drawing::default();
                self.stroke = None;
                self.textures.clear();
                self.failed_pages.clear();
                self.current_page = 0;
                self.fit_pending = true;
                self.scroll_reset = true;
            }
            Err(OpenError::PasswordRequired | OpenError::WrongPassword) => {
                self.dialog = Some(Dialog::Password {
                    path,
                    bytes,
                    input: String::new(),
                    wrong: password.is_some(),
                });
            }
            Err(e) => self.dialog = Some(Dialog::Error(format!("{}: {e}", file_name(&path)))),
        }
    }

    /// The open document, if any.
    pub fn document(&self) -> Option<&Document> {
        self.open.as_ref().map(|open| &open.document)
    }

    /// The finished strokes, oldest first.
    pub fn strokes(&self) -> &[Stroke] {
        self.drawing.strokes()
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Where page `index` was on screen in the last frame.
    pub fn page_rect(&self, index: usize) -> Option<Rect> {
        self.page_rects.get(index).copied()
    }

    /// Whether there are strokes that have not been saved.
    pub fn is_dirty(&self) -> bool {
        self.drawing.is_dirty()
    }

    /// The message in the status bar.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Does `then` now, or first asks whether unsaved strokes may be dropped.
    fn request(&mut self, ui: &Ui, then: Then) {
        if self.drawing.is_dirty() {
            self.dialog = Some(Dialog::Discard(then));
        } else {
            self.then(ui, then);
        }
    }

    fn then(&mut self, ui: &Ui, then: Then) {
        match then {
            Then::OpenDialog => {
                if let Some(path) = (self.dialogs.open)() {
                    self.open_path(&path);
                }
            }
            Then::Open(path) => self.open_path(&path),
            Then::Close => {
                self.allow_close = true;
                ui.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// Starts "Save As…": warns about digital signatures first, if the PDF has any.
    fn save(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        if self.drawing.strokes().is_empty() {
            return;
        }
        if open.document.signature_count > 0 {
            self.dialog = Some(Dialog::Signed);
        } else {
            self.choose_target_and_save();
        }
    }

    fn choose_target_and_save(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(target) = (self.dialogs.save)(&save::default_save_path(&open.path)) else {
            return;
        };
        if save::is_same_file(&open.path, &target) {
            let message = "signit never overwrites the PDF you opened. Choose another name.";
            self.dialog = Some(Dialog::Error(message.into()));
            return;
        }
        self.build_and_write(target, BTreeSet::new());
    }

    /// Builds the signed copy (with the form fields of `flatten` pages merged into the page) and
    /// writes it, unless some ink would be hidden: then the user decides.
    fn build_and_write(&mut self, target: PathBuf, flatten: BTreeSet<usize>) {
        let Some(open) = &self.open else {
            return;
        };
        let document = &open.document;
        let built = save::build(
            document.bytes(),
            document.password(),
            self.drawing.strokes(),
            &flatten,
        );
        match built {
            Ok(built) if built.hidden_ink_pages.is_empty() => self.write(&target, &built.bytes, ""),
            Ok(built) if flatten.is_empty() => {
                self.dialog = Some(Dialog::HiddenInk { target, built })
            }
            Ok(built) => {
                let note = format!(
                    " (some ink on page {} may still be hidden)",
                    page_list(&built.hidden_ink_pages)
                );
                self.write(&target, &built.bytes, &note);
            }
            Err(e) => self.dialog = Some(Dialog::Error(e.to_string())),
        }
    }

    fn write(&mut self, target: &Path, bytes: &[u8], note: &str) {
        match save::write_atomically(target, bytes) {
            Ok(()) => {
                self.drawing.mark_saved();
                self.status = format!("Saved {}{note}", file_name(target));
            }
            Err(e) => {
                let message = format!("{} could not be written: {e}", target.display());
                self.dialog = Some(Dialog::Error(message));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.guard_close(ui);
        if self.dialog.is_none() {
            self.take_dropped_file(ui);
            self.shortcuts(ui);
        }
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND))
            .show(ui, |ui| self.page_view(ui));
        self.show_dialog(ui);
        drop_overlay(ui);
        self.update_title(ui);
    }
}

impl App {
    /// Keeps the window open while there are unsaved strokes, and asks instead.
    fn guard_close(&mut self, ui: &Ui) {
        if ui.input(|i| i.viewport().close_requested())
            && self.drawing.is_dirty()
            && !self.allow_close
        {
            ui.send_viewport_cmd(ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Discard(Then::Close));
        }
    }

    fn take_dropped_file(&mut self, ui: &Ui) {
        let dropped = ui.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        let Some(path) = dropped else {
            return;
        };
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            self.request(ui, Then::Open(path));
        } else {
            let message = format!("{} is not a PDF.", file_name(&path));
            self.dialog = Some(Dialog::Error(message));
        }
    }

    fn shortcuts(&mut self, ui: &Ui) {
        // Plain letters are tool shortcuts only while no text field has the keyboard.
        let letters = !ui.ctx().text_edit_focused();
        let (mut open, mut save, mut undo, mut redo) = (false, false, false, false);
        let (mut zoom_in, mut zoom_out, mut fit, mut tool) = (false, false, false, None);
        ui.input_mut(|i| {
            open = i.consume_shortcut(&OPEN);
            save = i.consume_shortcut(&SAVE);
            // consume_shortcut ignores extra Shift, so redo must be checked before undo.
            redo = i.consume_shortcut(&REDO);
            undo = !redo && i.consume_shortcut(&UNDO);
            zoom_in = i.consume_shortcut(&ZOOM_IN) | i.consume_shortcut(&ZOOM_IN_PLUS);
            zoom_out = i.consume_shortcut(&ZOOM_OUT);
            fit = i.consume_shortcut(&FIT_WIDTH);
            if letters && i.consume_key(Modifiers::NONE, Key::P) {
                tool = Some(Tool::Pen);
            }
            if letters && i.consume_key(Modifiers::NONE, Key::E) {
                tool = Some(Tool::Eraser);
            }
        });
        if open {
            self.request(ui, Then::OpenDialog);
        }
        if save {
            self.save();
        }
        if undo {
            self.drawing.undo();
        }
        if redo {
            self.drawing.redo();
        }
        if zoom_in {
            self.zoom_by(ZOOM_STEP);
        }
        if zoom_out {
            self.zoom_by(1.0 / ZOOM_STEP);
        }
        if fit {
            self.fit_pending = true;
        }
        if let Some(tool) = tool {
            self.tool = tool;
        }
    }

    fn zoom_by(&mut self, factor: f32) {
        let from = self.zoom_request.unwrap_or(self.zoom);
        self.zoom_request = Some(view::clamp_zoom(from * factor));
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        // Shortcuts go in the tooltips: next to every button they make the toolbar too wide.
        let ctx = ui.ctx().clone();
        let shortcut = |s: &KeyboardShortcut| ctx.format_shortcut(s);
        ui.horizontal(|ui| {
            if ui.button("Open…").on_hover_text(shortcut(&OPEN)).clicked() {
                self.request(ui, Then::OpenDialog);
            }
            let can_save = self.open.is_some() && !self.drawing.strokes().is_empty();
            if ui
                .add_enabled(can_save, egui::Button::new("Save As…"))
                .on_hover_text(shortcut(&SAVE))
                .on_disabled_hover_text("Draw something first")
                .clicked()
            {
                self.save();
            }
            ui.separator();

            for (tool, label, key) in [(Tool::Pen, "Pen", "P"), (Tool::Eraser, "Eraser", "E")] {
                if ui
                    .selectable_label(self.tool == tool, label)
                    .on_hover_text(key)
                    .clicked()
                {
                    self.tool = tool;
                }
            }
            for (name, color) in INKS {
                if swatch(ui, name, color, self.color == color).clicked() {
                    self.color = color;
                    self.tool = Tool::Pen;
                }
            }
            let width = egui::Slider::new(&mut self.width, 0.5..=6.0)
                .step_by(0.25)
                .suffix(" pt");
            ui.add(width).on_hover_text("Pen width");
            ui.separator();

            let undo = ui.add_enabled(self.drawing.can_undo(), egui::Button::new("Undo"));
            if undo.on_hover_text(shortcut(&UNDO)).clicked() {
                self.drawing.undo();
            }
            let redo = ui.add_enabled(self.drawing.can_redo(), egui::Button::new("Redo"));
            if redo.on_hover_text(shortcut(&REDO)).clicked() {
                self.drawing.redo();
            }
            let has_strokes = !self.drawing.strokes().is_empty();
            let clear = ui.add_enabled(has_strokes, egui::Button::new("Clear all"));
            if clear.on_hover_text("Undo brings them back").clicked() {
                self.drawing.clear();
            }
            ui.separator();

            ui.add_enabled_ui(self.open.is_some(), |ui| {
                if ui.button("-").on_hover_text(shortcut(&ZOOM_OUT)).clicked() {
                    self.zoom_by(1.0 / ZOOM_STEP);
                }
                ui.label(format!("{:.0} %", self.zoom * 100.0));
                if ui.button("+").on_hover_text(shortcut(&ZOOM_IN)).clicked() {
                    self.zoom_by(ZOOM_STEP);
                }
                if ui
                    .button("Fit width")
                    .on_hover_text(shortcut(&FIT_WIDTH))
                    .clicked()
                {
                    self.fit_pending = true;
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(&self.status);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(open) = &self.open {
                    let pages = open.document.pages.len();
                    ui.label(format!("page {} / {pages}", self.current_page + 1));
                }
            });
        });
    }

    fn page_view(&mut self, ui: &mut Ui) {
        let Some(open) = &self.open else {
            self.empty_state(ui);
            return;
        };
        let sizes: Vec<Vec2> = open.document.pages.iter().map(|p| p.size).collect();
        let scroll_to = self.zoom_and_scroll(ui, &sizes);

        let output = ui
            .scope(|ui| {
                // A new offset beyond the previous frame's scroll range would otherwise count as
                // overscroll and spring back.
                if scroll_to.is_some() {
                    ui.spacing_mut().scroll.kinetic.rubber_band = false;
                }
                let mut area = egui::ScrollArea::both()
                    .id_salt("pages")
                    .auto_shrink([false, false])
                    // A drag draws; it never scrolls.
                    .scroll_source(ScrollSource {
                        drag: DragScroll::Never,
                        ..ScrollSource::default()
                    });
                if let Some(offset) = scroll_to {
                    area = area.scroll_offset(offset);
                }
                area.show_viewport(ui, |ui, viewport| {
                    self.drawn_offset = viewport.min.to_vec2();
                    self.pages(ui, viewport, &sizes);
                })
            })
            .inner;
        self.inner = Some(output.inner_rect);

        // The pointer may be released while the stroke's page is scrolled out of view.
        if self.stroke.is_some()
            && !ui.input(|i| i.pointer.primary_down())
            && let Some(stroke) = self.stroke.take()
        {
            self.drawing.add(stroke);
        }
        if ui.input(|i| i.time) - self.last_zoom_time < ZOOM_SETTLE {
            ui.ctx().request_repaint_after_secs(ZOOM_SETTLE as f32);
        }
    }

    /// Applies this frame's zooming (pinch, Cmd+scroll, keyboard, fit width) and returns the
    /// scroll offset to show, if it has to change.
    fn zoom_and_scroll(&mut self, ui: &Ui, sizes: &[Vec2]) -> Option<Vec2> {
        if std::mem::take(&mut self.fit_pending) {
            let width = self.inner.map_or(ui.available_width(), |r| r.width());
            self.zoom_request = Some(view::fit_width_zoom(sizes, width));
        }
        let (zoom_delta, pointer, time) =
            ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos(), i.time));
        let mut scroll_to = None;
        // A pinch or Cmd+scroll zooms around the pointer, the keyboard around the view's centre.
        let anchored = match self.inner {
            Some(inner) if zoom_delta != 1.0 && self.dialog.is_none() => pointer
                .filter(|p| inner.contains(*p))
                .map(|p| (self.zoom * zoom_delta, p, inner)),
            Some(inner) => self.zoom_request.map(|zoom| (zoom, inner.center(), inner)),
            None => None,
        };
        if let Some((zoom, anchor, inner)) = anchored {
            let zoom = view::clamp_zoom(zoom);
            scroll_to = Some(view::zoom_about(
                sizes,
                self.zoom,
                zoom,
                self.drawn_offset,
                anchor,
                inner,
            ));
            self.zoom = zoom;
            self.zoom_request = None;
            self.last_zoom_time = time;
            // Scrolling in the same frame would move the page under the pointer.
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        } else if let Some(zoom) = self.zoom_request.take() {
            // The first frame: there is no view to anchor to yet.
            self.zoom = zoom;
        }
        if std::mem::take(&mut self.scroll_reset) {
            scroll_to = Some(Vec2::ZERO);
        }
        scroll_to
    }

    fn empty_state(&self, ui: &mut Ui) {
        ui.centered_and_justified(|ui| {
            let text = match &self.pdfium_error {
                Some(e) => format!(
                    "PDFium library not found. Run ./scripts/fetch-pdfium.sh in the project \
                     folder, then start signit again.\n\n({e})"
                ),
                None => "Open a PDF (Cmd+O) or drop one here".into(),
            };
            ui.label(
                RichText::new(text)
                    .size(18.0)
                    .color(Color32::from_gray(0xE0)),
            );
        });
    }

    /// Lays out the pages in the scroll area's content and draws the visible ones.
    fn pages(&mut self, ui: &mut Ui, viewport: Rect, sizes: &[Vec2]) {
        let (rects, content_size) = view::layout(sizes, self.zoom, viewport.width());
        ui.set_min_size(content_size);
        let origin = ui.max_rect().min.to_vec2();
        self.page_rects = rects.iter().map(|r| r.translate(origin)).collect();
        self.current_page = view::nearest_page(&rects, viewport.center().y).unwrap_or(0);
        let visible = view::visible_pages(&rects, viewport);
        self.drop_far_textures(&visible);
        // PDFium renders on this thread: at most one page per frame keeps the UI responsive.
        let mut may_render = true;
        for index in visible {
            let screen = self.page_rects[index];
            let painter = ui.painter();
            painter.rect_filled(
                screen.translate(vec2(2.0, 3.0)),
                0.0,
                Color32::from_black_alpha(90),
            );
            painter.rect_filled(screen, 0.0, Color32::WHITE);
            self.update_texture(ui, index, sizes[index], &mut may_render);
            if let Some(texture) = self.textures.get(&index) {
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                ui.painter()
                    .image(texture.handle.id(), screen, uv, Color32::WHITE);
            }
            self.page_input(ui, index, screen);
            self.paint_strokes(ui, index, screen);
        }
    }

    /// Renders page `index` if it has no bitmap yet, or (once zooming has settled) if its bitmap
    /// is far from the size the current zoom needs.
    fn update_texture(&mut self, ui: &Ui, index: usize, size: Vec2, may_render: &mut bool) {
        let Some(open) = &self.open else {
            return;
        };
        if self.failed_pages.contains(&index) {
            return;
        }
        let (ppp, max_side, now) = ui.input(|i| (i.pixels_per_point, i.max_texture_side, i.time));
        let wanted = view::render_size(size, self.zoom, ppp, max_side);
        let settled = now - self.last_zoom_time >= ZOOM_SETTLE;
        let needed = match self.textures.get(&index) {
            None => true,
            Some(texture) => {
                settled && (texture.size[0] as f32 / wanted[0] as f32 - 1.0).abs() > 0.15
            }
        };
        if !needed {
            return;
        }
        if !*may_render {
            ui.ctx().request_repaint();
            return;
        }
        *may_render = false;
        match open.document.render(index, wanted) {
            Ok(image) => match self.textures.get_mut(&index) {
                Some(texture) => {
                    texture.handle.set(image, TextureOptions::LINEAR);
                    texture.size = wanted;
                }
                None => {
                    let handle = ui.ctx().load_texture(
                        format!("page {index}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    self.textures.insert(
                        index,
                        PageTexture {
                            handle,
                            size: wanted,
                        },
                    );
                }
            },
            Err(e) => {
                self.failed_pages.insert(index);
                self.status = format!("Page {} cannot be shown: {e}", index + 1);
            }
        }
        // Other pages may be waiting for their turn.
        ui.ctx().request_repaint();
    }

    /// Frees the bitmaps of pages more than two pages away from the visible ones.
    fn drop_far_textures(&mut self, visible: &Range<usize>) {
        let keep = visible.start.saturating_sub(2)..visible.end + 2;
        self.textures.retain(|index, _| keep.contains(index));
    }

    /// Pen and eraser input on page `index`, shown at `screen`.
    fn page_input(&mut self, ui: &Ui, index: usize, screen: Rect) {
        let Some(page) = self.document().map(|d| d.pages[index]) else {
            return;
        };
        let id = ui.make_persistent_id(("page", index));
        let response = ui
            .interact(screen, id, Sense::drag())
            .on_hover_and_drag_cursor(CursorIcon::Crosshair);
        if self.dialog.is_some() {
            return;
        }
        // Screen position -> fraction of the displayed page -> PDF user space.
        let to_user = |p: Pos2| {
            page.to_user
                .apply(((p - screen.min) / screen.size()).to_pos2())
        };
        // On a Mac trackpad a two-finger click is the secondary button: only the primary draws.
        let started = response.drag_started_by(PointerButton::Primary);
        let tap = if started { None } else { tap(ui, &response) };
        match self.tool {
            Tool::Pen => {
                if started {
                    self.stroke = Some(Stroke::new(index, self.color, self.width));
                }
                // Keep points half a screen point apart: closer ones add nothing visible.
                let min_distance = 0.5 / self.zoom;
                if let Some(stroke) = self.stroke.as_mut().filter(|s| s.page == index) {
                    for p in pointer_path(ui, started) {
                        stroke.push(to_user(p), min_distance);
                    }
                }
                if response.drag_stopped()
                    && let Some(stroke) = self.stroke.take_if(|s| s.page == index)
                {
                    self.drawing.add(stroke);
                }
                if let Some(p) = tap {
                    let mut dot = Stroke::new(index, self.color, self.width);
                    dot.push(to_user(p), 0.0);
                    self.drawing.add(dot);
                }
            }
            Tool::Eraser => {
                let radius = ERASER_RADIUS / self.zoom;
                if started {
                    self.eraser_at = None;
                }
                // Not `dragged()`: it is false on the release frame, whose moves count too.
                if started || self.eraser_at.is_some_and(|(page, _)| page == index) {
                    for p in pointer_path(ui, started).into_iter().map(to_user) {
                        // Sweep the whole way from the last position, so a quick swipe cannot
                        // jump over a thin stroke between two pointer events.
                        let from = self.eraser_at.map_or(p, |(_, at)| at);
                        for q in points_along(from, p, radius / 2.0) {
                            self.drawing.erase(index, q, radius);
                        }
                        self.eraser_at = Some((index, p));
                    }
                }
                if let Some(p) = tap {
                    self.drawing.erase(index, to_user(p), radius);
                }
                if response.drag_stopped() || tap.is_some() {
                    self.drawing.end_erase();
                    self.eraser_at = None;
                }
                if let Some(p) = response.hover_pos().filter(|_| response.hovered()) {
                    let outline = egui::Stroke::new(1.0, Color32::from_gray(0x60));
                    ui.painter().circle_stroke(p, ERASER_RADIUS, outline);
                }
            }
        }
    }

    fn paint_strokes(&self, ui: &Ui, index: usize, screen: Rect) {
        let Some(from_user) = self
            .document()
            .and_then(|d| d.pages[index].to_user.inverse())
        else {
            return;
        };
        let painter = ui
            .painter()
            .with_clip_rect(screen.intersect(ui.clip_rect()));
        let to_screen = |p: Pos2| screen.min + from_user.apply(p).to_vec2() * screen.size();
        let strokes = self.drawing.strokes().iter().chain(&self.stroke);
        for stroke in strokes.filter(|s| s.page == index) {
            let points: Vec<Pos2> = stroke
                .polyline(1.5 / self.zoom)
                .into_iter()
                .map(to_screen)
                .collect();
            let width = stroke.width * self.zoom;
            // epaint draws square line ends; discs make them round, as they are in the PDF.
            for end in [points.first(), points.last()].into_iter().flatten() {
                painter.circle_filled(*end, width / 2.0, stroke.color);
            }
            if points.len() > 1 {
                painter.add(Shape::line(points, egui::Stroke::new(width, stroke.color)));
            }
        }
    }

    fn show_dialog(&mut self, ui: &Ui) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut choice = None;
        let modal = egui::Modal::new(Id::unique("dialog")).show(ui.ctx(), |ui| {
            ui.set_max_width(420.0);
            match dialog {
                Dialog::Error(message) => {
                    ui.heading("Something went wrong");
                    ui.label(message.as_str());
                    if ui.button("OK").clicked() {
                        ui.close();
                    }
                }
                Dialog::Password {
                    path, input, wrong, ..
                } => {
                    ui.heading("Password needed");
                    ui.label(format!("{} is protected by a password.", file_name(path)));
                    if *wrong {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "That password is not correct.",
                        );
                    }
                    let label = ui.label("Password:");
                    let field = ui
                        .add(egui::TextEdit::singleline(input).password(true))
                        .labelled_by(label.id);
                    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    field.request_focus();
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() || entered {
                            choice = Some(Choice::Password);
                        }
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                    });
                }
                Dialog::Discard(_) => {
                    ui.heading("Discard your drawing?");
                    ui.label("Your strokes on this PDF have not been saved.");
                    ui.horizontal(|ui| {
                        if ui.button("Discard").clicked() {
                            choice = Some(Choice::Discard);
                        }
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                    });
                }
                Dialog::Signed => {
                    ui.heading("This PDF is digitally signed");
                    ui.label(
                        "Your saved copy will not carry a valid digital signature. \
                         The original file keeps its signature.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Continue").clicked() {
                            choice = Some(Choice::SaveSigned);
                        }
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                    });
                }
                Dialog::HiddenInk { built, .. } => {
                    ui.heading("Part of your drawing would be hidden");
                    ui.label(format!(
                        "On page {} some of your ink is under a form field, which PDF viewers \
                         draw on top of the page. Flattening merges the form fields of that \
                         page into the page, so they can no longer be edited.",
                        page_list(&built.hidden_ink_pages)
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Flatten form fields and save").clicked() {
                            choice = Some(Choice::Flatten);
                        }
                        if ui.button("Save anyway").clicked() {
                            choice = Some(Choice::SaveAnyway);
                        }
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                    });
                }
            }
        });
        match choice {
            Some(choice) => self.chosen(ui, choice),
            None if modal.should_close() => self.dialog = None,
            None => {}
        }
    }

    fn chosen(&mut self, ui: &Ui, choice: Choice) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        match (choice, dialog) {
            (
                Choice::Password,
                Dialog::Password {
                    path, bytes, input, ..
                },
            ) => {
                self.open_bytes(path, bytes, Some(&input));
            }
            (Choice::Discard, Dialog::Discard(then)) => self.then(ui, then),
            (Choice::SaveSigned, Dialog::Signed) => self.choose_target_and_save(),
            (Choice::Flatten, Dialog::HiddenInk { target, built }) => {
                self.build_and_write(target, built.hidden_ink_pages.into_iter().collect());
            }
            (Choice::SaveAnyway, Dialog::HiddenInk { target, built }) => {
                self.write(&target, &built.bytes, "");
            }
            // Each dialog only offers its own choices.
            (_, dialog) => self.dialog = Some(dialog),
        }
    }

    fn update_title(&mut self, ui: &Ui) {
        let title = match &self.open {
            Some(open) => {
                let edited = if self.drawing.is_dirty() {
                    " (edited)"
                } else {
                    ""
                };
                format!("{}{edited} - signit", file_name(&open.path))
            }
            None => "signit".to_owned(),
        };
        // Every Title command reaches the window, so send it only when it changes.
        if title != self.title {
            ui.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

/// The primary-button pointer positions of this frame, in order: the press, the moves while the
/// button is down, the release. Pass `started` on the frame the drag starts, so that moves
/// before the press (plain hovering) are skipped.
fn pointer_path(ui: &Ui, started: bool) -> Vec<Pos2> {
    let mut collecting = !started;
    ui.input(|i| {
        let mut path = Vec::new();
        for event in &i.events {
            match *event {
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    collecting = true;
                    path.push(pos);
                }
                Event::PointerMoved(pos) if collecting => path.push(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    ..
                } if collecting => {
                    path.push(pos);
                    collecting = false;
                }
                _ => {}
            }
        }
        path
    })
}

/// Where a primary press and release that both happened within this frame landed, if on the
/// widget. egui does not count such a quick tap as a drag, so it would otherwise draw nothing.
fn tap(ui: &Ui, response: &Response) -> Option<Pos2> {
    ui.input(|i| {
        let mut pressed_at = None;
        for event in &i.events {
            match *event {
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    pressed_at = Some(pos);
                }
                Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: false,
                    ..
                } if pressed_at.is_some() => {
                    return pressed_at;
                }
                _ => {}
            }
        }
        None
    })
    // `interact_rect` is clipped to the visible part of the scroll area.
    .filter(|p| response.interact_rect.contains(*p))
}

/// A round button showing an ink colour, ringed when selected. (Colour names as text are
/// unreadable for dark inks on the dark toolbar.)
fn swatch(ui: &mut Ui, name: &str, color: Color32, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, selected, name)
    });
    let painter = ui.painter();
    let centre = rect.center();
    painter.circle_filled(centre, 7.0, color);
    // A light rim, so black stays visible on the dark toolbar.
    painter.circle_stroke(
        centre,
        7.0,
        egui::Stroke::new(1.0, Color32::from_gray(0xB0)),
    );
    if selected {
        let ring = egui::Stroke::new(2.0, ui.visuals().selection.stroke.color);
        painter.circle_stroke(centre, 10.0, ring);
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(name)
}

/// Darkens the window while a file is dragged over it.
fn drop_overlay(ui: &Ui) {
    if ui.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let layer = egui::LayerId::new(egui::Order::Foreground, Id::unique("drop overlay"));
    let painter = ui.ctx().layer_painter(layer);
    let rect = ui.ctx().content_rect();
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(160));
    let font = FontId::proportional(22.0);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Drop the PDF to open it",
        font,
        Color32::WHITE,
    );
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// "2" or "1, 3" for 0-based page indices.
fn page_list(pages: &[usize]) -> String {
    let numbers: Vec<String> = pages.iter().map(|i| (i + 1).to_string()).collect();
    numbers.join(", ")
}
