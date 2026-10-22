//! nullpdf — a minimal PDF reader focused on freehand drawing/annotation.
//!
//! Single-key tools: `Q` navigate, `D` pen, `H` highlighter, `E` eraser, `T` text; Shift
//! draws straight lines, right-drag free-rotates (5° axis snap), wheel scrolls and
//! ctrl+wheel zooms. `/` searches the text, `B` the bookmarks, and `?` shows every shortcut.
//! Annotations (strokes + text) live in a `<pdf>.nullpdf.json` sidecar that is saved
//! automatically.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotation;
mod app;
mod icons;
mod keymap;
mod pdf;
mod search;
mod theme;
mod tools;
mod view;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    // Optional first CLI argument: a PDF to open on launch.
    let initial: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 900.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title("nullpdf")
            .with_app_id("nullpdf")
            .with_icon(icons::app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "nullpdf",
        native_options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, initial)))),
    )
}
