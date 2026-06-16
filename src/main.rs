//! nullpdf — a minimal PDF reader focused on freehand drawing/annotation.
//!
//! Modes: `Q` navigation (left-drag pan, right-drag free-rotate w/ 5° axis snap,
//! wheel scroll, ctrl+wheel zoom), `D` draw (left-drag freehand, hold Shift for a
//! straight line, `E` eraser), and `T` text (click to place/edit, drag to move). One
//! freehand tool doubles as a highlighter via the colour picker's alpha. Annotations
//! (strokes + text) live in a `<pdf>.nullpdf.json` sidecar.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotation;
mod app;
mod pdf;
mod tools;
mod view;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    // Optional first CLI argument: a PDF to open on launch.
    let initial: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 900.0])
            .with_title("nullpdf"),
        ..Default::default()
    };

    eframe::run_native(
        "nullpdf",
        native_options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, initial)))),
    )
}
