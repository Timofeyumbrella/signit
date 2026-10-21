//! The `signit [file.pdf]` command: opens the app, with the PDF if one is given.

use eframe::egui;
use pdf_draw::app::App;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: signit [file.pdf]";

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let path = match args.as_slice() {
        [] => None,
        [arg] if arg.as_os_str() == "-h" || arg.as_os_str() == "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        // A typo in the terminal is better reported there than in a window.
        [path] if !path.is_file() => {
            eprintln!("signit: {} is not a file", path.display());
            return ExitCode::FAILURE;
        }
        [path] => Some(path.clone()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("signit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: Option<PathBuf>) -> eframe::Result {
    #[allow(unused_mut)] // only changed on macOS
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("signit")
            .with_inner_size([1100.0, 850.0])
            .with_min_inner_size([600.0, 400.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    // winit's default macOS menu quits on Cmd+Q without asking the app first, which would skip
    // the "Discard your drawing?" question. Without it, egui turns Cmd+Q into a close request
    // that the app can cancel. (The menu's Hide and About items go with it.)
    #[cfg(target_os = "macos")]
    {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS as _;
            builder.with_default_menu(false);
        }));
    }
    eframe::run_native(
        "signit",
        options,
        Box::new(move |cc| {
            // Cmd+= / Cmd+- / Cmd+0 zoom the pages, not egui's whole UI.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            let mut app = App::new();
            if let Some(path) = path {
                app.open_path(&path);
            }
            Ok(Box::new(app))
        }),
    )
}
