//! The app driven headlessly with egui_kittest: pointer, keyboard and window events go in, the
//! app's state and the saved files are checked.

mod common;

use common::{fixture, has_color_near, render};
use eframe::egui::{
    self, Event, Key, Modifiers, PointerButton, Pos2, Rect, ViewportCommand, ViewportEvent,
    ViewportId, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use pdf_draw::app::{App, INKS, Tool};
use pdf_draw::pdf::Document;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A temporary folder, deleted when the test is done with it.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

/// A fresh folder for one test, with the fixture `name` written to `<name>.pdf` in it.
fn folder_with(test: &str, name: &str) -> (TempDir, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdf-draw-ui-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.pdf"));
    std::fs::write(&path, fixture(name)).unwrap();
    (TempDir(dir), path)
}

/// The app in a 1000 × 800 window with `path` open, after it has settled.
fn app_with(path: &Path) -> Harness<'static, App> {
    let path = path.to_path_buf();
    let mut h = Harness::builder()
        .with_size(vec2(1000.0, 800.0))
        .with_max_steps(40)
        .build_eframe(move |cc| {
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            let mut app = App::new();
            app.open_path(&path);
            app
        });
    h.run();
    assert!(h.state().document().is_some(), "the fixture did not open");
    h
}

fn page_rect(h: &Harness<App>, page: usize) -> Rect {
    h.state().page_rect(page).expect("page laid out")
}

/// Where the screen point `p` is in PDF user space, according to the app's last layout.
fn user_point(h: &Harness<App>, page: usize, p: Pos2) -> Pos2 {
    let rect = page_rect(h, page);
    let info = h.state().document().unwrap().pages[page];
    info.to_user.apply(((p - rect.min) / rect.size()).to_pos2())
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Drags along `path` with `which` button: press and the first moves in one frame, the rest and
/// the release in the next, as a real trackpad delivers them.
fn drag(h: &mut Harness<App>, path: &[Pos2], which: PointerButton) {
    h.hover_at(path[0]);
    h.run();
    let (first, rest) = path.split_at(path.len() / 2);
    let mut events = vec![Event::PointerMoved(first[0]), button(first[0], which, true)];
    events.extend(first[1..].iter().map(|&p| Event::PointerMoved(p)));
    h.input_mut().events.extend(events);
    h.step();
    let last = *path.last().unwrap();
    let mut events: Vec<Event> = rest.iter().map(|&p| Event::PointerMoved(p)).collect();
    events.push(button(last, which, false));
    h.input_mut().events.extend(events);
    h.step();
    h.run();
}

/// A zig-zag across the middle of page 0.
fn scribble_path(h: &Harness<App>) -> Vec<Pos2> {
    let c = page_rect(h, 0).center();
    [
        (-60.0, 0.0),
        (-30.0, -20.0),
        (0.0, 10.0),
        (30.0, -15.0),
        (60.0, 5.0),
    ]
    .iter()
    .map(|&(x, y)| c + vec2(x, y))
    .collect()
}

fn viewport_commands(h: &Harness<App>) -> Vec<ViewportCommand> {
    h.output().viewport_output[&ViewportId::ROOT]
        .commands
        .clone()
}

#[test]
fn a_drag_draws_a_stroke_where_the_pointer_went_and_does_not_scroll() {
    let (_dir, path) = folder_with("drag", "multipage");
    let mut h = app_with(&path);
    let before = page_rect(&h, 0);
    let path = scribble_path(&h);
    let expected: Vec<Pos2> = path.iter().map(|&p| user_point(&h, 0, p)).collect();
    drag(&mut h, &path, PointerButton::Primary);

    assert_eq!(page_rect(&h, 0), before, "the view scrolled");
    let strokes = h.state().strokes();
    assert_eq!(strokes.len(), 1);
    let stroke = &strokes[0];
    assert_eq!(
        (stroke.page, stroke.color, stroke.width),
        (0, INKS[0].1, 1.5)
    );
    assert_eq!(stroke.points.len(), expected.len());
    for (got, want) in stroke.points.iter().zip(&expected) {
        assert!(got.distance(*want) < 0.01, "{got:?} != {want:?}");
    }
    assert!(h.state().is_dirty());
}

#[test]
fn a_two_finger_click_drag_does_not_draw() {
    let (_dir, path) = folder_with("secondary", "letter");
    let mut h = app_with(&path);
    let path = scribble_path(&h);
    drag(&mut h, &path, PointerButton::Secondary);
    assert!(h.state().strokes().is_empty());
}

#[test]
fn a_tap_draws_a_dot() {
    let (_dir, path) = folder_with("tap", "letter");
    let mut h = app_with(&path);
    let p = page_rect(&h, 0).center();
    h.hover_at(p);
    h.run();
    // Press and release within one frame, as a quick trackpad tap can arrive.
    let p_user = user_point(&h, 0, p);
    h.input_mut().events.extend([
        button(p, PointerButton::Primary, true),
        button(p, PointerButton::Primary, false),
    ]);
    h.step();
    h.run();
    assert_eq!(h.state().strokes().len(), 1);
    assert_eq!(h.state().strokes()[0].points, [p_user]);
}

#[test]
fn pinch_zooms_around_the_pointer() {
    let (_dir, path) = folder_with("pinch", "multipage");
    let mut h = app_with(&path);
    let rect = page_rect(&h, 0);
    let pointer = rect.min + rect.size() * vec2(0.3, 0.4);
    h.hover_at(pointer);
    h.run();
    let zoom = h.state().zoom();
    h.event(Event::Zoom(1.5)); // what egui-winit sends for a trackpad pinch
    h.run();
    assert!((h.state().zoom() - zoom * 1.5).abs() < 1e-4);
    // The page point that was under the pointer is still there.
    let rect = page_rect(&h, 0);
    let now = rect.min + rect.size() * vec2(0.3, 0.4);
    assert!(
        now.distance(pointer) < 1.5,
        "{now:?} moved from {pointer:?}"
    );
}

#[test]
fn keyboard_undo_redo_and_eraser() {
    let (_dir, path) = folder_with("undo", "letter");
    let mut h = app_with(&path);
    let path = scribble_path(&h);
    drag(&mut h, &path, PointerButton::Primary);
    assert_eq!(h.state().strokes().len(), 1);

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(h.state().strokes().is_empty());
    assert!(!h.state().is_dirty(), "undone back to the opened state");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().strokes().len(), 1);

    // The eraser, dragged vertically across the stroke, removes it in one undoable step.
    h.key_press(Key::E);
    h.run();
    assert_eq!(h.state().tool(), Tool::Eraser);
    let c = page_rect(&h, 0).center();
    drag(
        &mut h,
        &[c + vec2(0.0, -40.0), c, c + vec2(0.0, 40.0)],
        PointerButton::Primary,
    );
    assert!(h.state().strokes().is_empty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().strokes().len(), 1);
}

#[test]
fn closing_with_unsaved_strokes_asks_first() {
    let (_dir, path) = folder_with("close", "letter");
    let mut h = app_with(&path);
    let path = scribble_path(&h);
    drag(&mut h, &path, PointerButton::Primary);

    let close = |h: &mut Harness<App>| {
        let input = h.input_mut().viewports.get_mut(&ViewportId::ROOT).unwrap();
        input.events.push(ViewportEvent::Close);
        h.step();
    };
    close(&mut h);
    assert!(viewport_commands(&h).contains(&ViewportCommand::CancelClose));
    h.run();
    h.get_by_label("Discard your drawing?");
    h.get_by_label("Discard").click();
    h.step();
    assert!(viewport_commands(&h).contains(&ViewportCommand::Close));
    // Now the close goes through.
    close(&mut h);
    assert!(!viewport_commands(&h).contains(&ViewportCommand::CancelClose));
}

#[test]
fn save_as_writes_a_new_pdf_with_the_ink() {
    let (dir, path) = folder_with("save", "letter");
    let original = std::fs::read(&path).unwrap();
    let mut h = app_with(&path);
    let target = dir.join("out.pdf");
    let suggested = std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let (target, suggested) = (target.clone(), suggested.clone());
        h.state_mut().dialogs.save = Box::new(move |s| {
            *suggested.lock().unwrap() = Some(s.to_path_buf());
            Some(target.clone())
        });
    }
    let path_drawn = scribble_path(&h);
    let rect = page_rect(&h, 0);
    drag(&mut h, &path_drawn, PointerButton::Primary);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run();

    assert_eq!(
        suggested.lock().unwrap().as_deref(),
        Some(dir.join("letter-signed.pdf").as_path())
    );
    assert!(
        h.state().status().starts_with("Saved out.pdf"),
        "{}",
        h.state().status()
    );
    assert!(!h.state().is_dirty());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "the original changed"
    );
    let saved = Document::open(std::fs::read(&target).unwrap(), None).unwrap();
    let image = render(&saved, 0);
    let middle = ((path_drawn[2] - rect.min) / rect.size()).to_pos2();
    assert!(
        has_color_near(&image, middle, INKS[0].1, 2),
        "no ink in the saved file"
    );
}

#[test]
fn saving_over_the_opened_file_is_refused() {
    let (_dir, path) = folder_with("same", "letter");
    let original = std::fs::read(&path).unwrap();
    let mut h = app_with(&path);
    let same = path.clone();
    h.state_mut().dialogs.save = Box::new(move |_| Some(same.clone()));
    let path_drawn = scribble_path(&h);
    drag(&mut h, &path_drawn, PointerButton::Primary);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run();
    h.get_by_label_contains("never overwrites");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(h.state().is_dirty());
}

#[test]
fn ink_over_a_form_field_offers_to_flatten() {
    let (dir, path) = folder_with("field", "annotated");
    let mut h = app_with(&path);
    let target = dir.join("out.pdf");
    {
        let target = target.clone();
        h.state_mut().dialogs.save = Box::new(move |_| Some(target.clone()));
    }
    // The field covers x 300..540, y 380..410 pt (from the bottom) of the 612 × 792 page.
    let rect = page_rect(&h, 0);
    let at = |x: f32, y: f32| rect.min + rect.size() * vec2(x / 612.0, (792.0 - y) / 792.0);
    let path_drawn = [
        at(330.0, 395.0),
        at(380.0, 400.0),
        at(430.0, 390.0),
        at(500.0, 396.0),
    ];
    drag(&mut h, &path_drawn, PointerButton::Primary);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run();
    h.get_by_label("Part of your drawing would be hidden");
    assert!(!target.exists(), "written before the user chose");
    h.get_by_label("Flatten form fields and save").click();
    h.run();

    let saved = Document::open(std::fs::read(&target).unwrap(), None).unwrap();
    let middle = ((path_drawn[1] - rect.min) / rect.size()).to_pos2();
    assert!(has_color_near(&render(&saved, 0), middle, INKS[0].1, 2));
}

#[test]
fn a_password_protected_pdf_asks_for_the_password() {
    let (_dir, path) = folder_with("password", "letter");
    std::fs::copy(common::fixture_file("user-password"), &path).unwrap();
    let path_clone = path.clone();
    let mut h = Harness::builder()
        .with_size(vec2(1000.0, 800.0))
        .with_max_steps(40)
        .build_eframe(move |_| {
            let mut app = App::new();
            app.open_path(&path_clone);
            app
        });
    h.run();
    assert!(h.state().document().is_none());
    h.get_by_label("Password needed");

    let type_and_open = |h: &mut Harness<App>, password: &str| {
        h.event(Event::Text(password.into()));
        h.run();
        h.get_by_label("Open").click();
        h.run();
    };
    type_and_open(&mut h, "wrong");
    h.get_by_label("That password is not correct.");
    type_and_open(&mut h, "user123");
    assert_eq!(h.state().document().map(|d| d.pages.len()), Some(1));
}

#[test]
fn dropping_a_pdf_opens_it() {
    #[derive(Debug)]
    struct Dropped(PathBuf);
    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    let (dir, path) = folder_with("drop", "letter");
    let mut h = app_with(&path);
    let other = dir.join("multipage.pdf");
    std::fs::write(&other, fixture("multipage")).unwrap();
    h.input_mut().dropped_files.push(Arc::new(Dropped(other)));
    h.run();
    assert_eq!(h.state().document().map(|d| d.pages.len()), Some(3));
}

#[test]
fn the_toolbar_zooms_and_fits_the_width() {
    let (_dir, path) = folder_with("zoom", "letter");
    let mut h = app_with(&path);
    let fit = h.state().zoom();
    // Fit width: the page fills the view minus the gaps on both sides.
    let width = page_rect(&h, 0).width();
    assert!(
        (width - (1000.0 - 2.0 * pdf_draw::view::GAP)).abs() < 1.0,
        "{width}"
    );
    h.get_by_label("+").click();
    h.run();
    assert!((h.state().zoom() - fit * 1.25).abs() < 1e-4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    h.run();
    assert!((h.state().zoom() - fit).abs() < 1e-4);
}
