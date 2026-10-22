# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`nullpdf` is a minimal desktop PDF reader focused on freehand drawing/annotation, built
with `eframe`/`egui` (immediate-mode GUI) and `pdfium-render` (system libpdfium binding).
Single binary, no async, no networking. The UI is deliberately minimal (floating chrome over
the document) and fully driven by single-key shortcuts.

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
`render_check` also verifies that detail tiles register exactly with a whole-page render;
`export_check` covers strokes, highlights, dots, embedded-font text and glyph outlines.

**Runtime requirement:** the system `libpdfium` must be installed and discoverable
(`Pdfium::bind_to_system_library`). If it's missing the app still launches but the welcome
screen explains that libpdfium couldn't be loaded, and documents can't be opened. File
dialogs (`rfd`) use the XDG desktop portal on Linux; without one, the app says so instead
of failing silently (drag-and-drop and the CLI argument still work).

## Architecture

The whole app is one `eframe::App` whose `ui()` runs every frame: reload check → drops /
close request → keyboard → canvas (pointer gestures → camera glide + clamp → page textures
→ paint) → text editor overlay → floating chrome → autosave → repaint scheduling. State
lives in the `App` struct; there is no separate event loop or message bus. Every user
command is an `Action` (`keymap.rs`) run through `App::dispatch`, whether it came from a
key or a toolbar click.

Module responsibilities:
- `app.rs` — the `App`: owns all state, document lifecycle (open, live reload, autosave,
  export, safe close), styles, undo/redo, camera targets (fit, paging, zoom, rotation) and
  `dispatch`. Start here. Its submodules (child modules, so they see `App`'s private fields):
  - `app/input.rs` — keyboard → actions (incl. Space tap-vs-hold), pointer gestures
    (pan/rotate/draw/erase/text) from raw events, wheel/pinch/trackpad input, eraser hit-tests.
  - `app/canvas.rs` — painting: page shadows/textures/tiles, strokes (own tessellator with
    round caps), text, live preview, hover feedback, custom cursors, rotation readout.
  - `app/text.rs` — text layout and hit-testing, the in-place editor, and the text/glyph
    data the exporter burns in.
  - `app/chrome.rs` — floating toolbar, status line, toasts, `?` sheet, welcome screen,
    drop overlay.
- `keymap.rs` — `Action`, the `BINDINGS` table (single source of truth for both key
  handling and the `?` cheat sheet) and the cheat-sheet layout.
- `view.rs` — `Camera` (pan/zoom/free-rotation, clamp, glide), `Layout` (continuous
  vertical page stack), rotation snapping.
- `pdf.rs` — `PdfDoc`: pdfium loading, the page texture cache (exact-resolution,
  budgeted, detail tiles, eviction), flattened-PDF export.
- `annotation.rs` — `Stroke`/`TextAnnotation`/`AnnotationSet` data model, JSON sidecar I/O,
  `chaikin` smoothing, `simplify` (RDP), and `History` (undo/redo).
- `tools.rs` — `Tool`, palette and size ladders, `Brush`, and in-flight `Gesture` /
  `TextEditing` state.
- `theme.rs` — design tokens and the egui style; `icons.rs` — painted vector icons and the
  window icon.

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

The floating chrome covers parts of the viewport, so fitting, paging and clamping work on
`App::safe_rect()` (the viewport minus toolbar/status space when they're shown).

### Interaction model

Tools (`tools.rs`): **Q** Navigate (left-drag pan), **D** Pen, **H** Highlighter (the same
freehand brush with its own translucent colour and width), **E** Eraser (whole marks; press
again to return to the previous drawing tool), **T** Text (click empty space to add text,
click existing text to edit with the caret at the click, drag it to move). **1–7** pick a
palette colour, **[ ]** step the size of whatever the toolbar's size control shows (pen or
highlighter width, font size, eraser radius). Picking a colour while navigating or erasing
picks up the drawing tool.

Anywhere: right-drag rotates (5° axis snap, Shift = 15° steps), middle-drag or Space+drag
pans, wheel scrolls through the page stack (doc space, regardless of rotation), Ctrl+wheel
or pinch zooms at the pointer. Shift while drawing makes a straight line, snapped to the
page's horizontal/vertical within 5°. A tap draws a dot or erases what's under the eraser.

View and paging keys (all animated via a camera *glide*): `+ −` zoom, `0` reset (upright,
fit width), `F` fit page, `R`/Shift+R rotate 90°, `↑ ↓` scroll, `← →`/PgUp/PgDn page,
Space/Shift+Space screen, Home/End. Fit width/page is a *mode* (`Fit`): resizing the window
or toggling the chrome re-fits until the user zooms by hand. Files: `O` open, `S` save,
`X` export (Ctrl+O/S/E too). `Z`/Shift+Z undo/redo (Ctrl variants too); an undo of an
off-screen change glides it into view. `?` shows every binding, `Tab` hides the chrome,
`Esc` cancels the gesture/editor or returns to Navigate.

While a text editor has focus, bare keys are typing: `keymap::lookup` only honours Ctrl
bindings then (not undo/redo, which belong to the editor). In the editor, Enter commits,
Shift+Enter starts a new line, Esc cancels, a click elsewhere commits.

Annotations are a tagged `Annotation` enum (`Stroke` | `Text`) sharing one id space, so
undo/redo, erase, and the sidecar treat both uniformly. Text is stored in normalised page
coords (top-left anchor) like strokes; it's painted via a rotated `epaint::TextShape` and a
frameless `egui::Area` overlay during editing that sits exactly where the committed text
draws (the overlay stays axis-aligned even when the page is rotated).
`Edit::EditedText { before, after }` captures in-place content/style/move edits as a single
undo step; `Edit::Erased` holds a whole erase sweep (with list indices, replayed in reverse
on undo so the stacking order is restored).

### Rendering

- **Page textures** (`pdf.rs`): a page gets a texture as soon as it scrolls into view;
  once the view has *settled* (`SETTLE_TIME` unchanged), visible pages are re-rendered at
  exactly their on-screen resolution, and upright pages are drawn snapped so texels map 1:1
  to physical pixels (crisp text). Whole-page textures are capped at `MAX_PAGE_SIDE`;
  beyond that a **detail tile** of just the visible region is rendered on top (not for
  documents with interactive forms — pdfium only draws form data without a transform).
  Rendering is time-budgeted per frame (`FRAME_BUDGET`), nearest pages first; textures
  outside the visible range ±1 page, or over `CACHE_BUDGET_PX`, are evicted. Don't render
  per-frame.
- **Strokes** use `canvas::paint_polyline`, a tessellator with round caps and joins and a
  1px feathered edge, so the screen matches the exported PDF's round-capped paths.
- **Idle means zero frames**: nothing polls. A watcher thread (`FileWatch`) stats the open
  PDF and only requests a repaint when its mtime changes; animations, pending renders,
  toast fades and autosave schedule their own repaints.

### Persistence

Annotations load from and **autosave** to a `<pdf>.nullpdf.json` sidecar
(`AnnotationSet::sidecar_path`) shortly after each change (`AUTOSAVE_DELAY`), never
modifying the base PDF; `S` saves immediately, and opening another file or closing saves
first. Writes are atomic (temp file + rename). A sidecar that exists but can't be parsed is
never silently overwritten: it's renamed to `.bak` on the first save. If saving fails at
close, the first close is refused with an explanation and a second close quits. Nothing is
written for a document that never had annotations.

"Export" (`PdfDoc::export_flattened`) burns annotations into a *copy* (never over the open
file): strokes as stroked path objects with round caps (single-point strokes as filled
dots), text as real selectable text in egui's own proportional font (embedded TrueType),
and glyphs that egui draws from its symbol/emoji fallback fonts as filled outlines.
Positions come from egui's text layout, so the export matches the screen.
`Stroke.id` is runtime-only (`#[serde(skip)]`) and reassigned deterministically on load;
undo/redo keys off it.

### Things to know when editing

- **`Pdfium` is leaked** (`Box::leak`) to get a `'static` document, sidestepping the
  self-referential lifetime of holding library + borrowing doc together. Exactly one is
  leaked for the app's lifetime — don't try to store both in a struct normally.
- **Keep the chrome non-focusable.** Toolbar widgets use `Sense::CLICK` (no `FOCUSABLE`)
  and `raw_input_hook` strips Tab, so no widget can take keyboard focus and swallow the
  single-key shortcuts. Typing is detected with `ctx.text_edit_focused()`, not
  `egui_wants_keyboard_input()` (which is true for *any* focused widget).
- **Pointer gestures come from raw events** (`Event::PointerButton`/`PointerMoved`), not
  egui's drag detection, which only reports a drag after ~6px — that would clip the start
  of every stroke and make taps impossible. Use the modifiers carried by the button event
  at release (Shift and the button often come up in the same frame).
- **WYSIWYG smoothing**: freehand input is Chaikin-smoothed (2 iterations) on finalize, and
  the in-progress preview applies the *same* smoothing so committed ink matches the preview;
  `simplify` then drops redundant points with a sub-pixel tolerance.
- **Rotation snapping** must snap a *raw* accumulated angle (`snap_rotation`), never the
  already-snapped rotation, or slow drags can't leave an axis.
- **Auto-reload** keeps in-memory annotations and the old page pixels (`adopt_textures`)
  until fresh renders replace them. It's deferred mid-gesture; a partial write fails the
  re-open and is retried shortly. Annotations on pages beyond a shrunken page count are
  kept but not drawn or hit-tested.
- egui's `ctx.input(|i| …)` holds the context lock: read what you need inside the closure,
  then act after it (`handle_keyboard` collects `Action`s and dispatches them afterwards).
