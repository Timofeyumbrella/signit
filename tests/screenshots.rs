//! Renders the app to PNG files in `target/screenshots/` for a human to look at:
//!
//!     cargo test --test screenshots -- --ignored
//!
//! Ignored by default because it needs the GPU and checks nothing by itself.

mod common;

use common::fixture;
use eframe::egui::{Event, Modifiers, PointerButton, Pos2, Rect, vec2};
use egui_kittest::Harness;
use pdf_draw::app::App;
use std::path::{Path, PathBuf};

fn harness(open: Option<PathBuf>) -> Harness<'static, App> {
    let mut h = Harness::builder()
        .with_size(vec2(1100.0, 850.0))
        .with_pixels_per_point(2.0) // like a Retina screen
        .with_os(eframe::egui::os::OperatingSystem::Mac)
        .with_max_steps(40)
        .wgpu()
        .build_eframe(move |cc| {
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            let mut app = App::new();
            if let Some(path) = &open {
                app.open_path(path);
            }
            app
        });
    // Let the GPU report its real texture limit (the harness starts at 2048).
    h.input_mut().max_texture_side = Some(8192);
    h.run();
    h
}

fn save(h: &mut Harness<App>, name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    h.render().unwrap().save(&path).unwrap();
    path
}

/// A cursive-looking scribble along the "Signed: ____" line of the contract fixture, whose
/// line runs at about x 115–265 pt, 150 pt above the bottom of a 612 × 792 pt page.
fn signature(page: Rect) -> Vec<Pos2> {
    let at = |x: f32, y: f32| page.min + page.size() * vec2(x / 612.0, (792.0 - y) / 792.0);
    (0..=120)
        .map(|i| {
            let t = i as f32 / 120.0;
            let x = 122.0 + 130.0 * t + 9.0 * (t * 9.0 * std::f32::consts::TAU).sin();
            let y = 158.0 + 11.0 * (t * 4.5 * std::f32::consts::TAU).sin() * (1.0 - 0.4 * t);
            at(x, y)
        })
        .collect()
}

#[test]
#[ignore = "writes PNGs for a human to look at; needs the GPU"]
fn screenshots() {
    let mut h = harness(None);
    let empty = save(&mut h, "1-empty.png");

    let dir = std::env::temp_dir().join(format!("pdf-draw-shots-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("contract.pdf");
    std::fs::write(&path, fixture("multipage")).unwrap();
    let mut h = harness(Some(path));
    let opened = save(&mut h, "2-opened.png");

    // Scroll so the signature line of page 1 is in view, then sign it.
    let page = h.state().page_rect(0).unwrap();
    h.hover_at(page.center());
    h.event(Event::MouseWheel {
        unit: eframe::egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -page.height() * 0.45),
        phase: eframe::egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
        source: eframe::egui::MouseWheelSource::Trackpad,
    });
    h.run();
    let points = signature(h.state().page_rect(0).unwrap());
    h.hover_at(points[0]);
    h.run();
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.input_mut().events.push(press(points[0], true));
    for chunk in points.chunks(6) {
        h.input_mut()
            .events
            .extend(chunk.iter().map(|&p| Event::PointerMoved(p)));
        h.step();
    }
    h.input_mut()
        .events
        .push(press(*points.last().unwrap(), false));
    h.step();
    h.hover_at(*points.last().unwrap() + vec2(40.0, 30.0));
    h.run();
    assert_eq!(h.state().strokes().len(), 1);
    let signed = save(&mut h, "3-signed.png");
    std::fs::remove_dir_all(&dir).unwrap();
    println!(
        "{}\n{}\n{}",
        empty.display(),
        opened.display(),
        signed.display()
    );
}
