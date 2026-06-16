# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`nullpdf` is a minimal desktop PDF reader focused on freehand drawing/annotation, built
with `eframe`/`egui` (immediate-mode GUI) and `pdfium-render` (system libpdfium binding).
Single binary, no async, no networking.

## Commands

```sh
cargo run                       # launch GUI
cargo run -- path/to/file.pdf   # launch and open a PDF immediately
cargo build --release           # release build (windows_subsystem="windows" so no console on Win)
cargo clippy                    # lint
cargo fmt                       # format

# Headless smoke tests (verify the pdfium pipeline without a GUI/Wayland screenshot):
cargo run --example render_check -- <in.pdf> <page_index> <out.png>
cargo run --example export_check -- <in.pdf> <out.pdf> <out.png>
```

There is no unit-test suite. The two `examples/` are the verification harness — they
deliberately mirror the exact pdfium calls in `src/pdf.rs`, so if you change rendering or
export, update the corresponding example and run it to confirm against a real PDF.

**Runtime requirement:** the system `libpdfium` must be installed and discoverable
(`Pdfium::bind_to_system_library`). If it's missing the app still launches but shows
"Failed to load libpdfium" and can't open documents.

## Architecture

The whole app is one `eframe::App` (`src/app.rs`) whose `ui()` runs every frame:
poll-for-reload → keyboard → toolbar/statusbar panels → central canvas (input → clamp →
paint). State lives in the `App` struct; there is no separate event loop or message bus.

Module responsibilities:
- `app.rs` — the `App`: owns all state, wires everything together, handles input, paints
  the rotated page quads + strokes. Start here.
- `view.rs` — `Camera` (pan/zoom/free-rotation) and `Layout` (continuous vertical page stack).
- `pdf.rs` — `PdfDoc`: pdfium loading, per-page texture cache, flattened-PDF export.
- `annotation.rs` — `Stroke`/`AnnotationSet` data model, JSON sidecar I/O, `chaikin`
  smoothing, and `History` (undo/redo).
- `tools.rs` — `Mode` (Nav/Draw), eraser flag, in-progress `ActiveStroke`.

### Coordinate spaces (the central concept)

Three spaces, converted via `view.rs`. Get these right or strokes land in the wrong place:
- **page-normalised** `[0..1, 0..1]`, top-left origin — how every `Stroke` is *stored*.
  Being resolution/zoom-independent is what lets annotations survive reload and re-align to
  an externally-edited base PDF (as long as page count/order is unchanged).
- **doc** — PDF points, all pages stacked vertically, centred on `x = 0` (`Layout::page_rect`).
- **screen** — egui pixels, via the rotation-invariant `Camera` (`doc_to_screen` /
  `screen_to_doc`).

The camera stores the doc point shown at the viewport *centre* plus zoom + free rotation;
panning/zooming/rotating all preserve that invariant. Note pdfium page space is y-up
(bottom-left origin), so export flips: `y_pt = (1.0 - n[1]) * h` — see `to_pt` in `pdf.rs`.

### Interaction model

Three modes (`tools.rs`): **Q** = Nav (left-drag pan, wheel scroll, ctrl+wheel zoom),
**D** = Draw (left-drag freehand, hold **Shift** for a straight line, **E** toggles eraser),
**T** = Text (click empty space to add text, click existing text to re-edit, drag it to move).
Right-drag rotates in *any* mode (5° axis snap). There is one freehand tool; it doubles
as a highlighter purely via the colour picker's alpha (low alpha = translucent highlight).
The width slider doubles as the font-size slider while in Text mode. Letter shortcuts
(Q/D/T/E) are suppressed when a text editor has focus (`egui_wants_keyboard_input`).
Shortcuts: Ctrl+O open, Ctrl+S save sidecar, Ctrl+E export, Ctrl+Z/Ctrl+Shift+Z/Ctrl+Y undo/redo.

Annotations are a tagged `Annotation` enum (`Stroke` | `Text`) sharing one id space, so
undo/redo, erase, and the sidecar treat both uniformly. Text is stored in normalised page
coords (top-left anchor) like strokes; it's painted via a rotated `epaint::TextShape` and a
live `egui::Area` overlay during editing (the overlay stays axis-aligned even when the page
is rotated). `Edit::EditedText { before, after }` captures in-place content/style/move edits
as a single undo step. Export burns text as real selectable PDF text (built-in Helvetica),
positioned by dropping ~0.8·size from the top-left anchor to the baseline (`ASCENT_RATIO`).

### Persistence

Annotations auto-load/save to a `<pdf>.nullpdf.json` sidecar (`AnnotationSet::sidecar_path`),
never modifying the base PDF. "Export" (`PdfDoc::export_flattened`) burns strokes into a
*copy* as real stroked path objects. `Stroke.id` is runtime-only (`#[serde(skip)]`) and
reassigned deterministically on load; undo/redo keys off it.

### Things to know when editing

- **`Pdfium` is leaked** (`Box::leak`) to get a `'static` document, sidestepping the
  self-referential lifetime of holding library + borrowing doc together. Exactly one is
  leaked for the app's lifetime — don't try to store both in a struct normally.
- **Texture cache** (`pdf.rs`) re-renders a page only when zoom crosses a `SCALE_STEP`
  bucket, capped at `MAX_SCALE`. Don't render per-frame.
- **WYSIWYG smoothing**: freehand input is Chaikin-smoothed (2 iterations) on finalize, and
  the in-progress preview applies the *same* smoothing so committed ink matches the preview.
- **Auto-reload**: the open PDF's mtime is polled (`RELOAD_POLL`); external edits re-load
  pages while keeping in-memory annotations. Skipped mid-stroke; a partial write fails the
  re-open and is retried next tick.
- egui input must be read inside `ctx.input(|i| …)`, which can't borrow `&mut self`; the
  pattern in `handle_keyboard` is to set flags inside the closure and act after it.
