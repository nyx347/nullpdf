//! The eframe application: owns all state, runs the per-frame loop, and implements every
//! action the keymap and toolbar can trigger.
//!
//! Frame order: poll for reload → drops/close → keyboard → canvas (pointer gestures →
//! camera glide + clamp → page textures → paint) → text editor overlay → chrome → search →
//! autosave. Submodules split the work: `input` (keyboard + pointer gestures), `canvas`
//! (painting the document), `text` (text annotations: layout, editing, export runs),
//! `chrome` (the floating toolbar, status line, toasts, cheat sheet and welcome screen),
//! `find` (the find bar and text search) and `bookmarks` (the bookmark palette).

mod bookmarks;
mod canvas;
mod chrome;
mod find;
mod input;
mod text;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use egui::{Color32, Pos2, Rect, Sense, Vec2};
use pdfium_render::prelude::Pdfium;

use crate::annotation::{Annotation, AnnotationSet, History, SidecarLoad};
use crate::keymap::Action;
use crate::pdf::{PageNeed, PdfDoc};
use crate::search::TextSearch;
use crate::theme;
use crate::tools::{
    step_size, Brush, Gesture, StyleTarget, TextEditing, Tool, ERASER_SIZES, FONT_SIZES,
    HIGHLIGHT_ALPHA, PALETTE, STROKE_SIZES,
};
use crate::view::{next_axis, Camera, Layout};

/// How often the watcher thread checks the open PDF's mtime for external changes.
const RELOAD_POLL: Duration = Duration::from_millis(750);

/// Unsaved annotations are written this long (s) after the last change.
const AUTOSAVE_DELAY: f64 = 0.8;
/// After a failed save, wait this long (s) before trying again.
const SAVE_RETRY: f64 = 5.0;

/// The view counts as settled (worth re-rendering crisply) after this long (s) unchanged.
const SETTLE_TIME: f64 = 0.12;

/// Keyboard zoom step.
const ZOOM_STEP: f32 = 1.25;
/// Arrow-key scroll step, in screen pixels.
const ARROW_STEP: f32 = 64.0;
/// Fraction of the view a Space/Shift+Space "screen" scroll moves.
const SCREEN_STEP: f32 = 0.88;
/// "Fit width" never zooms past this (screen px per point ≈ 135% of actual size), so pages
/// on very wide windows stay readable instead of ballooning.
const MAX_FIT_ZOOM: f32 = 1.8;
/// Breathing room (screen px) kept around a fitted page.
const FIT_MARGIN: f32 = 20.0;
/// Space reserved at the top of the view for the floating toolbar, and at the bottom for the
/// status line, when chrome is visible.
const TOOLBAR_SPACE: f32 = theme::MARGIN + theme::BUTTON + 8.0 + 8.0;
const STATUS_SPACE: f32 = theme::MARGIN + 26.0 + 4.0;

/// Info toasts stay this long (s); errors longer. They fade in quickly and out gently.
const TOAST_TIME: f64 = 2.2;
const ERROR_TOAST_TIME: f64 = 6.0;
const TOAST_FADE_IN: f64 = 0.15;
const TOAST_FADE_OUT: f64 = 0.35;

/// Zoom shown as 100% in the status line: one PDF point = one CSS pixel at 96 dpi, the
/// "actual size" other viewers call 100%.
const ACTUAL_SIZE_ZOOM: f32 = 96.0 / 72.0;

/// A fit the view keeps re-applying — when the window is resized or the toolbar shows or
/// hides — until the user zooms by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fit {
    Width,
    Page,
}

/// Wakes the UI when the open PDF's modification time changes, so an idle app renders no
/// frames at all instead of repainting just to poll. The UI thread still does the actual
/// reload (`App::check_reload`); this only stats the file and requests a repaint.
struct FileWatch {
    path: Arc<Mutex<Option<PathBuf>>>,
}

impl FileWatch {
    fn spawn(ctx: egui::Context) -> Self {
        let path = Arc::new(Mutex::new(None::<PathBuf>));
        let shared = Arc::clone(&path);
        let _ = std::thread::Builder::new()
            .name("nullpdf-watch".into())
            .spawn(move || {
                let mut last: Option<(PathBuf, Option<SystemTime>)> = None;
                loop {
                    std::thread::sleep(RELOAD_POLL);
                    let Some(p) = shared.lock().ok().and_then(|g| g.clone()) else {
                        last = None;
                        continue;
                    };
                    let mtime = current_mtime(&p);
                    if last
                        .as_ref()
                        .is_some_and(|(lp, lm)| *lp == p && *lm != mtime)
                    {
                        ctx.request_repaint();
                    }
                    last = Some((p, mtime));
                }
            });
        Self { path }
    }

    fn watch(&self, path: Option<PathBuf>) {
        if let Ok(mut g) = self.path.lock() {
            *g = path;
        }
    }
}

struct Toast {
    text: String,
    error: bool,
    born: f64,
}

impl Toast {
    fn ttl(&self) -> f64 {
        if self.error {
            ERROR_TOAST_TIME
        } else {
            TOAST_TIME
        }
    }
}

pub struct App {
    ctx: egui::Context,
    /// Leaked, app-lifetime Pdfium binding. `None` if the system library failed to load.
    pdfium: Option<&'static Pdfium>,
    /// Why Pdfium failed to load, shown on the welcome screen.
    pdfium_error: Option<String>,
    pdf: Option<PdfDoc>,
    pdf_path: Option<PathBuf>,
    /// Last-modified time of `pdf_path` when last loaded, for change detection.
    pdf_mtime: Option<SystemTime>,
    watch: FileWatch,
    /// A changed file couldn't be re-opened yet (likely mid-write); retry shortly.
    reload_retry: bool,
    layout: Layout,
    set: AnnotationSet,
    history: History,

    cam: Camera,
    /// Target of an in-flight animated camera move (keyboard zoom/scroll/page/rotate).
    /// Direct manipulation cancels it; further keys build on the target, not the
    /// mid-animation camera, so rapid presses accumulate exactly.
    glide: Option<Camera>,
    /// True until the first frame after opening has fitted the view.
    needs_fit: bool,
    /// The fit in force, and the visible area it was computed for.
    fit: Option<Fit>,
    fit_safe: Rect,
    prev_cam: Camera,
    /// When the camera last moved (for "settled" re-rendering).
    view_changed_at: f64,
    /// Canvas rect of the latest frame.
    viewport: Rect,

    tool: Tool,
    /// The drawing tool (Pen or Highlighter) the eraser toggles back to, and what colour
    /// and size changes apply to while navigating.
    last_draw: Tool,
    pen: Brush,
    highlighter: Brush,
    /// Font size for new text (text uses the pen's colour).
    text_size: f32,
    /// Eraser radius in screen pixels.
    eraser_px: f32,

    gesture: Gesture,
    /// The pointer button that started `gesture`.
    gesture_button: Option<egui::PointerButton>,
    /// The open text editor, if any.
    text_edit: Option<TextEditing>,
    /// Space is held (spring-loaded pan) and whether it has been used to pan; a plain tap
    /// scrolls a screen on release instead.
    space_down: bool,
    space_used: bool,
    /// Tab is down (it is consumed in `raw_input_hook`, so egui never moves focus).
    tab_down: bool,
    /// Actions raised outside the keyboard handler (Tab, from `raw_input_hook`), run with
    /// this frame's keys.
    queued: Vec<Action>,

    /// Annotations changed since the last successful save.
    dirty: bool,
    changed_at: f64,
    /// When the last save failed (for retry back-off) and why (to avoid repeating toasts).
    save_failed_at: Option<f64>,
    save_error: Option<String>,
    /// The existing sidecar could not be read; it is backed up before the first overwrite.
    sidecar_unreadable: bool,
    /// A close was refused because annotations couldn't be saved; the next close proceeds.
    close_armed: bool,

    chrome_visible: bool,
    help_open: bool,
    /// The find bar is open (`/`).
    find_open: bool,
    /// The find query, kept when the bar closes so it can be reopened or repeated (`N`).
    find_query: String,
    find_case: bool,
    /// The search for `find_query` while the bar is open (`None` while the query is empty).
    search: Option<TextSearch>,
    /// The find field should take keyboard focus, its text selected, this frame.
    find_focus: bool,
    /// The bookmark palette (`B`), while open.
    bookmarks: Option<bookmarks::Palette>,
    toast: Option<Toast>,
    /// Window title last sent, to avoid resending every frame.
    title: String,
    /// `ctx.input(|i| i.time)` of the current frame.
    now: f64,
    /// Modifier keys held this frame.
    mods: egui::Modifiers,
    /// Wheel travel accumulated over the size control, stepped in whole rungs.
    size_wheel: f32,
    /// Unsnapped angle and last event time of a trackpad/touch rotation in progress.
    twist: Option<(f32, f64)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let (pdfium, pdfium_error) = match PdfDoc::init_pdfium() {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            pdfium,
            pdfium_error,
            pdf: None,
            pdf_path: None,
            pdf_mtime: None,
            watch: FileWatch::spawn(cc.egui_ctx.clone()),
            reload_retry: false,
            layout: Layout::default(),
            set: AnnotationSet::default(),
            history: History::default(),
            cam: Camera::default(),
            glide: None,
            needs_fit: false,
            fit: None,
            fit_safe: Rect::NOTHING,
            prev_cam: Camera::default(),
            view_changed_at: 0.0,
            viewport: Rect::ZERO,
            tool: Tool::Nav,
            last_draw: Tool::Pen,
            pen: Brush {
                color: PALETTE[1].1,
                size: 3.0,
            },
            highlighter: Brush {
                color: with_alpha(PALETTE[3].1, HIGHLIGHT_ALPHA),
                size: 14.0,
            },
            text_size: 14.0,
            eraser_px: 8.0,
            gesture: Gesture::None,
            gesture_button: None,
            text_edit: None,
            space_down: false,
            space_used: false,
            tab_down: false,
            queued: Vec::new(),
            dirty: false,
            changed_at: 0.0,
            save_failed_at: None,
            save_error: None,
            sidecar_unreadable: false,
            close_armed: false,
            chrome_visible: true,
            help_open: false,
            find_open: false,
            find_query: String::new(),
            find_case: false,
            search: None,
            find_focus: false,
            bookmarks: None,
            toast: None,
            title: String::new(),
            now: 0.0,
            mods: egui::Modifiers::NONE,
            size_wheel: 0.0,
            twist: None,
        };

        if let Some(path) = initial {
            app.open_path(&path);
        }
        app
    }

    // --- feedback ----------------------------------------------------------------

    fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: false,
            born: self.now,
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: true,
            born: self.now,
        });
    }

    /// Short-lived confirmation of a style/tool change, only when the toolbar that would
    /// otherwise show it is hidden.
    fn hint(&mut self, text: impl Into<String>) {
        if !self.chrome_visible {
            self.notify(text);
        }
    }

    fn file_name(&self) -> String {
        self.pdf_path
            .as_deref()
            .and_then(Path::file_name)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    // --- documents ---------------------------------------------------------------

    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
        if let Some(dir) = self.pdf_path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        let shown = std::time::Instant::now();
        match dialog.pick_file() {
            Some(path) => self.open_path(&path),
            None => self.dialog_failed(shown, "open a file"),
        }
    }

    /// The system file dialog returns nothing both when cancelled and when it couldn't be
    /// shown at all (e.g. no xdg-desktop-portal). Nobody cancels within a blink, so an
    /// instant "cancel" gets an explanation instead of silence.
    fn dialog_failed(&mut self, shown: std::time::Instant, what: &str) {
        if shown.elapsed() < Duration::from_millis(150) {
            self.error(format!(
                "Couldn't show a file dialog to {what} — is xdg-desktop-portal installed? \
                 (PDFs can also be dropped on the window or passed on the command line)"
            ));
        }
    }

    fn open_path(&mut self, path: &Path) {
        let Some(pdfium) = self.pdfium else {
            self.error("Can't open PDFs: libpdfium isn't available");
            return;
        };
        // Leaving the current document: finish edits (keeping a stroke in progress) and
        // persist them first.
        self.commit_text();
        self.finish_gesture(true);
        if !self.flush_save() {
            return;
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let doc = match PdfDoc::open(pdfium, path) {
            Ok(doc) => doc,
            Err(e) => {
                self.error(format!("Couldn't open {name}: {e}"));
                return;
            }
        };
        let page_count = doc.sizes.len();
        self.layout = Layout::new(doc.sizes.clone());
        self.pdf = Some(doc);
        self.pdf_path = Some(path.to_path_buf());
        self.pdf_mtime = current_mtime(path);
        self.watch.watch(Some(path.to_path_buf()));
        self.reload_retry = false;
        self.sidecar_unreadable = false;
        self.toast = None;

        self.set = match AnnotationSet::load_for(path) {
            SidecarLoad::Loaded(mut s) => {
                if s.page_count != page_count {
                    self.error(format!(
                        "{name} now has {page_count} pages (annotations were made on {}) — \
                         some may be misplaced",
                        s.page_count
                    ));
                }
                s.pdf_file = name.clone();
                s.page_count = page_count;
                s
            }
            SidecarLoad::Missing => AnnotationSet::new(name.clone(), page_count),
            SidecarLoad::Unreadable(e) => {
                self.sidecar_unreadable = true;
                self.error(format!(
                    "Couldn't read saved annotations ({e}); the file will be kept as .bak"
                ));
                AnnotationSet::new(name.clone(), page_count)
            }
        };
        self.history = History::default();
        self.dirty = false;
        self.save_failed_at = None;
        self.save_error = None;
        self.close_armed = false;
        self.needs_fit = true;
        self.glide = None;
        self.gesture = Gesture::None;
        self.text_edit = None;
        self.close_find();
        self.bookmarks = None;
    }

    /// Re-open the PDF (pages only) when the file changes on disk, preserving the
    /// in-memory annotations (normalised, so they re-align), the view, and — until fresh
    /// renders replace them — the old page pixels, so a rebuild-and-reload never flashes.
    /// Deferred while a gesture is in progress, so drawing is never disrupted.
    fn check_reload(&mut self) {
        let (Some(pdfium), Some(path)) = (self.pdfium, self.pdf_path.clone()) else {
            return;
        };
        let Some(new_mtime) = current_mtime(&path) else {
            return;
        };
        if self.pdf_mtime == Some(new_mtime) {
            self.reload_retry = false;
            return;
        }
        if !self.gesture.is_none() {
            self.reload_retry = true;
            return;
        }
        // File changed (or appeared): try to re-open. A partial write will fail here;
        // we leave mtime untouched and retry shortly, once the writer finishes.
        let Ok(mut doc) = PdfDoc::open(pdfium, &path) else {
            self.reload_retry = true;
            return;
        };
        self.reload_retry = false;
        let page_count = doc.sizes.len();
        if let Some(old) = self.pdf.take() {
            doc.adopt_textures(old);
        }
        self.layout = Layout::new(doc.sizes.clone());
        self.pdf = Some(doc);
        self.pdf_mtime = Some(new_mtime);
        // The text and outline may have changed too.
        if self.search.is_some() {
            self.restart_search(false);
        }
        self.refilter_bookmarks();
        if page_count != self.set.page_count {
            self.error(format!(
                "Reloaded — now {page_count} pages (was {}); annotations may be misplaced",
                self.set.page_count
            ));
            self.set.page_count = page_count;
        } else {
            self.notify("Reloaded — the file changed on disk");
        }
    }

    fn export_dialog(&mut self) {
        let (Some(pdfium), Some(src)) = (self.pdfium, self.pdf_path.clone()) else {
            return;
        };
        self.commit_text();
        self.finish_gesture(true);
        let default_name = src
            .file_stem()
            .map(|s| format!("{}-annotated.pdf", s.to_string_lossy()))
            .unwrap_or_else(|| "annotated.pdf".to_string());
        let mut dialog = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(default_name);
        if let Some(dir) = src.parent() {
            dialog = dialog.set_directory(dir);
        }
        let shown = std::time::Instant::now();
        let Some(mut out) = dialog.save_file() else {
            self.dialog_failed(shown, "export");
            return;
        };
        if out.extension().is_none() {
            out.set_extension("pdf");
        }
        if same_file(&out, &src) {
            self.error("Export to a different file — the open PDF stays untouched");
            return;
        }
        let (runs, shapes, font) = self.export_text();
        let strokes = &self.set.strokes;
        match PdfDoc::export_flattened(pdfium, &src, &out, strokes, &runs, &shapes, &font) {
            Ok(()) => {
                let name = out.file_name().map(|s| s.to_string_lossy().into_owned());
                self.notify(format!("Exported {}", name.unwrap_or_default()));
            }
            Err(e) => self.error(format!("Export failed: {e}")),
        }
    }

    /// Write the sidecar now. A document with no annotations and no sidecar yet writes
    /// nothing (no clutter next to PDFs that were only read).
    fn save_now(&mut self) -> Result<(), String> {
        let Some(path) = self.pdf_path.clone() else {
            return Ok(());
        };
        let sidecar = AnnotationSet::sidecar_path(&path);
        if self.set.is_empty() && !sidecar.exists() {
            self.dirty = false;
            return Ok(());
        }
        if self.sidecar_unreadable && sidecar.exists() {
            let mut bak = sidecar.clone().into_os_string();
            bak.push(".bak");
            std::fs::rename(&sidecar, &bak).map_err(|e| e.to_string())?;
        }
        self.sidecar_unreadable = false;
        self.set.page_count = self.layout.len();
        self.set.save_for(&path).map_err(|e| e.to_string())?;
        self.dirty = false;
        self.save_failed_at = None;
        self.save_error = None;
        Ok(())
    }

    /// Explicit save (S / Ctrl+S).
    fn save(&mut self) {
        self.commit_text();
        self.finish_gesture(true);
        match self.save_now() {
            Ok(()) => self.notify("Annotations saved"),
            Err(e) => self.error(format!("Couldn't save annotations: {e}")),
        }
    }

    /// Save if there are unsaved changes; on failure, say so and return `false`.
    fn flush_save(&mut self) -> bool {
        if !self.dirty {
            return true;
        }
        match self.save_now() {
            Ok(()) => true,
            Err(e) => {
                self.error(format!("Couldn't save annotations: {e}"));
                false
            }
        }
    }

    /// Autosave shortly after edits settle (never mid-gesture or mid-typing).
    fn autosave_tick(&mut self) {
        if !self.dirty || !self.gesture.is_none() || self.text_edit.is_some() {
            return;
        }
        if self.now - self.changed_at < AUTOSAVE_DELAY {
            return;
        }
        if self
            .save_failed_at
            .is_some_and(|t| self.now - t < SAVE_RETRY)
        {
            return;
        }
        if let Err(e) = self.save_now() {
            self.save_failed_at = Some(self.now);
            if self.save_error.as_deref() != Some(e.as_str()) {
                self.error(format!("Couldn't save annotations: {e}"));
                self.save_error = Some(e);
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.changed_at = self.now;
        self.close_armed = false;
    }

    // --- styles --------------------------------------------------------------------

    /// What colour/size changes apply to right now.
    fn style_target(&self) -> StyleTarget {
        if self.text_edit.is_some() || self.tool == Tool::Text {
            return StyleTarget::Text;
        }
        match self.tool {
            Tool::Highlighter => StyleTarget::Highlighter,
            Tool::Pen => StyleTarget::Pen,
            _ if self.last_draw == Tool::Highlighter => StyleTarget::Highlighter,
            _ => StyleTarget::Pen,
        }
    }

    /// The colour the toolbar shows as selected.
    fn current_color(&self) -> Color32 {
        match self.style_target() {
            StyleTarget::Text => self
                .text_edit
                .as_ref()
                .map_or(self.pen.color, |te| te.color),
            StyleTarget::Highlighter => self.highlighter.color,
            StyleTarget::Pen => self.pen.color,
        }
    }

    /// Apply a colour to the current style target. Picking a colour while navigating or
    /// erasing picks up the drawing tool, ready to draw with it.
    fn set_color(&mut self, color: Color32, from_palette: bool) {
        if matches!(self.tool, Tool::Nav | Tool::Eraser) && self.text_edit.is_none() {
            self.set_tool(self.last_draw);
        }
        match self.style_target() {
            StyleTarget::Text => {
                match self.text_edit.as_mut() {
                    Some(te) => {
                        te.color = color;
                        // A new text's style is the current text style; an existing text's
                        // edit leaves the pen alone.
                        if te.editing_id.is_none() {
                            self.pen.color = color;
                        }
                    }
                    None => self.pen.color = color,
                }
            }
            StyleTarget::Highlighter => {
                self.highlighter.color = if from_palette {
                    with_alpha(color, HIGHLIGHT_ALPHA)
                } else {
                    color
                };
            }
            StyleTarget::Pen => self.pen.color = color,
        }
    }

    /// The size the toolbar shows and `[`/`]` step: (value, ladder, unit).
    fn current_size(&self) -> (f32, &'static [f32], &'static str) {
        if self.tool == Tool::Eraser && self.text_edit.is_none() {
            return (self.eraser_px, &ERASER_SIZES, "px");
        }
        match self.style_target() {
            StyleTarget::Text => (
                self.text_edit
                    .as_ref()
                    .map_or(self.text_size, |te| te.size_pt),
                &FONT_SIZES,
                "pt",
            ),
            StyleTarget::Highlighter => (self.highlighter.size, &STROKE_SIZES, "pt"),
            StyleTarget::Pen => (self.pen.size, &STROKE_SIZES, "pt"),
        }
    }

    fn set_size(&mut self, value: f32) {
        if self.tool == Tool::Eraser && self.text_edit.is_none() {
            self.eraser_px = value;
            return;
        }
        match self.style_target() {
            StyleTarget::Text => match self.text_edit.as_mut() {
                Some(te) => {
                    te.size_pt = value;
                    if te.editing_id.is_none() {
                        self.text_size = value;
                    }
                }
                None => self.text_size = value,
            },
            StyleTarget::Highlighter => self.highlighter.size = value,
            StyleTarget::Pen => self.pen.size = value,
        }
    }

    fn step_size(&mut self, dir: i32) {
        let (value, ladder, unit) = self.current_size();
        let next = step_size(ladder, value, dir);
        self.set_size(next);
        self.hint(format!("Size {} {unit}", format_size(next)));
    }

    fn set_tool(&mut self, tool: Tool) {
        if tool == self.tool {
            return;
        }
        self.finish_gesture(true);
        if tool != Tool::Text {
            self.commit_text();
        }
        if tool.draws() {
            self.last_draw = tool;
        }
        self.tool = tool;
        self.hint(tool.name());
    }

    // --- history -------------------------------------------------------------------

    fn undo(&mut self) {
        // Mid-stroke (or mid-erase/move), undo means "not this": just drop the gesture.
        if matches!(
            self.gesture,
            Gesture::Draw(_) | Gesture::Erase { .. } | Gesture::TextDrag { .. }
        ) {
            self.cancel_gesture();
            return;
        }
        self.commit_text();
        self.cancel_gesture();
        match self.history.undo(&mut self.set) {
            Some(touched) => {
                self.mark_dirty();
                self.reveal(&touched);
            }
            None => self.notify("Nothing to undo"),
        }
    }

    fn redo(&mut self) {
        self.commit_text();
        self.cancel_gesture();
        match self.history.redo(&mut self.set) {
            Some(touched) => {
                self.mark_dirty();
                self.reveal(&touched);
            }
            None => self.notify("Nothing to redo"),
        }
    }

    /// If none of `anns` is on screen, glide the view to them, so an undo/redo is never
    /// an invisible change on some other page.
    fn reveal(&mut self, anns: &[Annotation]) {
        let Some(bbox) = anns
            .iter()
            .filter(|a| a.page() < self.layout.len())
            .map(|a| match a {
                Annotation::Stroke(s) => self.stroke_bbox_doc(s),
                Annotation::Text(t) => self.text_bbox_doc(t),
            })
            .reduce(Rect::union)
        else {
            return;
        };
        let view = self.cam.doc_bbox(self.safe_rect(), self.viewport);
        if view.intersects(bbox) {
            return;
        }
        let mut target = self.glide_target();
        if target.is_upright() {
            // Minimal move: only along an axis where the change is out of view, so the
            // page doesn't also jump sideways.
            let shift = |lo: f32, hi: f32, vlo: f32, vhi: f32| {
                if hi < vlo || lo > vhi {
                    (lo + hi - vlo - vhi) * 0.5
                } else {
                    0.0
                }
            };
            target.center.x += shift(bbox.min.x, bbox.max.x, view.min.x, view.max.x);
            target.center.y += shift(bbox.min.y, bbox.max.y, view.min.y, view.max.y);
        } else {
            let safe = self.safe_rect().center();
            target.center = target.center_for(bbox.center(), safe, self.viewport);
        }
        self.glide_to(target);
    }

    // --- camera ----------------------------------------------------------------------

    /// The part of the viewport not covered by floating chrome. The find bar shares the
    /// status line's strip, and keeps it clear even with the toolbar hidden.
    fn safe_rect(&self) -> Rect {
        let mut r = self.viewport;
        if self.pdf.is_some() {
            if self.chrome_visible {
                r.min.y += TOOLBAR_SPACE;
            }
            if self.chrome_visible || self.find_open {
                r.max.y -= STATUS_SPACE;
            }
        }
        if r.height() < 40.0 {
            return self.viewport;
        }
        r
    }

    fn glide_target(&self) -> Camera {
        self.glide.unwrap_or(self.cam)
    }

    fn glide_to(&mut self, mut target: Camera) {
        target.clamp_to(&self.layout, self.viewport, self.safe_rect());
        self.glide = Some(target);
    }

    /// The page under the centre of the visible area (what the status line calls "current").
    fn current_page(&self) -> usize {
        self.page_under(&self.cam)
    }

    /// Where reading is, `down` (0..1) of the way down the visible area: the page there,
    /// and how far down it (points; negative above its top edge).
    fn reading_position(&self, down: f32) -> (usize, f32) {
        let safe = self.safe_rect();
        let at = Pos2::new(safe.center().x, safe.top() + safe.height() * down);
        let doc = self.glide_target().screen_to_doc(at, self.viewport);
        let page = self.layout.page_near_y(doc.y).unwrap_or(0);
        (page, doc.y - self.layout.page_rect(page).top())
    }

    /// The page under the centre of the visible area as seen by `cam`.
    fn page_under(&self, cam: &Camera) -> usize {
        let c = cam.screen_to_doc(self.safe_rect().center(), self.viewport);
        self.layout.page_near_y(c.y).unwrap_or(0)
    }

    /// Zoom that fits page `i`'s width (as rotated by `cam`) into the visible area.
    fn fit_width_zoom(&self, cam: &Camera, i: usize) -> f32 {
        let p = self.layout.pages[i];
        let unit = Camera { zoom: 1.0, ..*cam };
        let ext = unit.screen_extent(Vec2::new(p.w, p.h));
        ((self.safe_rect().width() - 2.0 * FIT_MARGIN) / ext.x).min(MAX_FIT_ZOOM)
    }

    /// Camera that centres page `i` in the visible area, zoomed to fit it whole.
    fn fit_page_camera(&self, base: Camera, i: usize) -> Camera {
        let p = self.layout.pages[i];
        let safe = self.safe_rect();
        let unit = Camera { zoom: 1.0, ..base };
        let ext = unit.screen_extent(Vec2::new(p.w, p.h));
        let zoom = ((safe.width() - 2.0 * FIT_MARGIN) / ext.x)
            .min((safe.height() - 2.0 * FIT_MARGIN) / ext.y);
        let mut cam = Camera { zoom, ..base };
        cam.center = cam.center_for(
            self.layout.page_rect(i).center(),
            safe.center(),
            self.viewport,
        );
        cam
    }

    /// Where the camera goes to "show" page `i` at the current zoom and rotation: a page
    /// that fits is centred; a taller one is shown from its top edge. Horizontally, a page
    /// wider than the view keeps the current pan.
    fn page_camera(&self, base: Camera, i: usize) -> Camera {
        let safe = self.safe_rect();
        let r = self.layout.page_rect(i);
        let ext = base.screen_extent(r.size());
        let current = base.doc_to_screen(r.center(), self.viewport);
        let x = if ext.x <= safe.width() {
            safe.center().x
        } else {
            current.x
        };
        let y = if ext.y <= safe.height() {
            safe.center().y
        } else {
            safe.top() + FIT_MARGIN + ext.y * 0.5
        };
        let mut cam = base;
        cam.center = cam.center_for(r.center(), Pos2::new(x, y), self.viewport);
        cam.clamp_to(&self.layout, self.viewport, safe);
        cam
    }

    /// Glide to a link target: page `i`, with the point `offset` points down it at the top
    /// of the view (or, rotated, in its centre) when the page doesn't fit whole.
    fn show_destination(&mut self, i: usize, offset: Option<f32>) {
        let mut cam = self.page_camera(self.glide_target(), i);
        let r = self.layout.page_rect(i);
        let safe = self.safe_rect();
        if let Some(y) = offset.filter(|_| cam.screen_extent(r.size()).y > safe.height()) {
            let target = Pos2::new(r.center().x, r.top() + y);
            let goal = if cam.is_upright() {
                let x = cam.doc_to_screen(target, self.viewport).x;
                Pos2::new(x, safe.top() + FIT_MARGIN)
            } else {
                safe.center()
            };
            cam.center = cam.center_for(target, goal, self.viewport);
        }
        self.glide_to(cam);
    }

    /// `base` re-fitted to its current page: the whole page for [`Fit::Page`]; for
    /// [`Fit::Width`], the page's width, keeping the reading position and (upright) centring
    /// the page across the view.
    fn fitted(&self, base: Camera, fit: Fit) -> Camera {
        let page = self.page_under(&base);
        match fit {
            Fit::Page => self.fit_page_camera(base, page),
            Fit::Width => {
                let safe = self.safe_rect();
                let centre = base.screen_to_doc(safe.center(), self.viewport);
                let mut cam = base;
                cam.zoom = self.fit_width_zoom(&base, page);
                let anchor = if base.is_upright() {
                    Pos2::new(self.layout.page_rect(page).center().x, centre.y)
                } else {
                    centre
                };
                cam.center = cam.center_for(anchor, safe.center(), self.viewport);
                cam
            }
        }
    }

    /// Enter fit mode `fit` and glide there.
    fn fit_to(&mut self, fit: Fit, base: Camera) {
        self.fit = Some(fit);
        self.fit_safe = self.safe_rect();
        let cam = self.fitted(base, fit);
        self.glide_to(cam);
    }

    /// Re-apply the fit in force if the visible area changed size (window resize, toolbar
    /// shown/hidden). Instant, so the page tracks the window edge while resizing.
    fn keep_fit(&mut self) {
        let safe = self.safe_rect();
        let Some(fit) = self.fit else {
            return;
        };
        if safe == self.fit_safe || self.layout.is_empty() || !self.gesture.is_none() {
            return;
        }
        self.fit_safe = safe;
        let mut cam = self.fitted(self.glide_target(), fit);
        cam.clamp_to(&self.layout, self.viewport, safe);
        self.cam = cam;
        self.glide = None;
    }

    /// Initial view of a freshly opened document: upright, first page fitted to the width
    /// and shown from its top.
    fn initial_view(&mut self) {
        self.needs_fit = false;
        let mut cam = Camera::default();
        cam.zoom = self.fit_width_zoom(&cam, 0);
        let mut cam = self.page_camera(cam, 0);
        cam.clamp_to(&self.layout, self.viewport, self.safe_rect());
        self.cam = cam;
        self.prev_cam = cam;
        self.glide = None;
        self.fit = Some(Fit::Width);
        self.fit_safe = self.safe_rect();
    }

    /// Upright + fit width on the current page, keeping what's at the centre of the view
    /// in place vertically.
    fn reset_view(&mut self) {
        let mut base = self.glide_target();
        base.rotation = 0.0;
        self.fit_to(Fit::Width, base);
    }

    fn goto_page(&mut self, dir: i32) {
        if self.layout.is_empty() {
            return;
        }
        let base = self.glide_target();
        let tol = 1.0 / base.zoom;
        let n = self.layout.len();
        let target = |i: usize| self.page_camera(base, i);
        let pick = match dir {
            i32::MIN => Some(0),
            i32::MAX => Some(n - 1),
            d if d > 0 => (0..n).find(|&i| target(i).center.y > base.center.y + tol),
            _ => (0..n)
                .rev()
                .find(|&i| target(i).center.y < base.center.y - tol),
        };
        if let Some(i) = pick {
            let cam = target(i);
            self.glide_to(cam);
        }
    }

    /// Scroll by `screen_px` along the reading direction (doc y), animated.
    fn scroll_by(&mut self, screen_px: f32) {
        let mut cam = self.glide_target();
        cam.center.y += screen_px / cam.zoom;
        self.glide_to(cam);
    }

    /// Scroll by a fraction of the visible area's extent along doc y.
    fn scroll_screen(&mut self, dir: f32) {
        let cam = self.glide_target();
        let extent = cam.doc_half_extent(self.safe_rect().size()).y * 2.0 * cam.zoom;
        self.scroll_by(dir * extent * SCREEN_STEP);
    }

    fn zoom_step(&mut self, factor: f32) {
        self.fit = None;
        let mut cam = self.glide_target();
        cam.zoom_around(factor, self.safe_rect().center(), self.viewport);
        self.glide_to(cam);
    }

    /// Quarter-turn to the next axis; a fit in force is re-applied for the new orientation.
    fn rotate_step(&mut self, dir: f32) {
        let mut cam = self.glide_target();
        cam.rotation = next_axis(cam.rotation, dir);
        if let Some(fit) = self.fit {
            cam = self.fitted(cam, fit);
        }
        self.glide_to(cam);
    }

    /// Advance the camera glide and clamp, and note when the view last changed.
    fn update_camera(&mut self, dt: f32) {
        if let Some(target) = self.glide {
            if !self.cam.step_toward(&target, dt) {
                self.glide = None;
            }
        }
        if !self.layout.is_empty() {
            let safe = self.safe_rect();
            self.cam.clamp_to(&self.layout, self.viewport, safe);
        }
        if self.cam != self.prev_cam {
            self.view_changed_at = self.now;
            self.prev_cam = self.cam;
        }
    }

    fn view_settled(&self) -> bool {
        self.glide.is_none() && self.now - self.view_changed_at > SETTLE_TIME
    }

    /// Ask the page cache for what this frame shows; returns whether rendering is pending.
    fn prepare_pages(&mut self, ctx: &egui::Context) -> bool {
        let settled = self.view_settled();
        let Some(pdf) = self.pdf.as_mut() else {
            return false;
        };
        let viewport = self.viewport;
        let ppp = ctx.pixels_per_point();
        let view = self.cam.doc_bbox(viewport, viewport);
        let visible = self.layout.pages_in(view);
        let needs: Vec<PageNeed> = visible
            .clone()
            .filter_map(|i| {
                let r = self.layout.page_rect(i);
                let shown = r.intersect(view);
                if !shown.is_positive() {
                    return None;
                }
                let local = shown.translate(-r.min.to_vec2());
                let centre = self.cam.doc_to_screen(r.center(), viewport);
                Some(PageNeed {
                    index: i,
                    scale: self.cam.zoom * ppp,
                    visible: local,
                    priority: centre.distance(viewport.center()),
                })
            })
            .collect();
        let keep = visible.start.saturating_sub(1)..(visible.end + 1).min(self.layout.len());
        pdf.prepare(ctx, &needs, keep, settled)
    }

    // --- actions -------------------------------------------------------------------

    fn dispatch(&mut self, action: Action) {
        // The cheat sheet is a quick reference: using any shortcut from it dismisses it.
        if self.help_open && !matches!(action, Action::Help | Action::Cancel) {
            self.help_open = false;
        }
        if action.needs_document() && self.pdf.is_none() {
            return;
        }
        match action {
            Action::Tool(t) => self.set_tool(t),
            Action::ToggleEraser => {
                let t = if self.tool == Tool::Eraser {
                    self.last_draw
                } else {
                    Tool::Eraser
                };
                self.set_tool(t);
            }
            Action::Color(i) => {
                self.set_color(PALETTE[i].1, true);
                self.hint(PALETTE[i].0);
            }
            Action::SizeDown => self.step_size(-1),
            Action::SizeUp => self.step_size(1),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::ZoomIn => self.zoom_step(ZOOM_STEP),
            Action::ZoomOut => self.zoom_step(1.0 / ZOOM_STEP),
            Action::ResetView => self.reset_view(),
            Action::FitPage => self.fit_to(Fit::Page, self.glide_target()),
            Action::RotateCw => self.rotate_step(1.0),
            Action::RotateCcw => self.rotate_step(-1.0),
            Action::ScrollUp => self.scroll_by(-ARROW_STEP),
            Action::ScrollDown => self.scroll_by(ARROW_STEP),
            Action::PrevPage => self.goto_page(-1),
            Action::NextPage => self.goto_page(1),
            Action::FirstPage => self.goto_page(i32::MIN),
            Action::LastPage => self.goto_page(i32::MAX),
            Action::ScreenUp => self.scroll_screen(-1.0),
            Action::ScreenDown => self.scroll_screen(1.0),
            Action::Find => self.open_find(),
            Action::FindNext => self.find_step(1),
            Action::FindPrev => self.find_step(-1),
            Action::Bookmarks => self.toggle_bookmarks(),
            Action::Open => self.open_dialog(),
            Action::Save => self.save(),
            Action::Export => self.export_dialog(),
            Action::Help => self.help_open = !self.help_open,
            Action::ToggleChrome => {
                self.chrome_visible = !self.chrome_visible;
                if !self.chrome_visible {
                    self.notify("Toolbar hidden — Tab brings it back");
                }
            }
            Action::Cancel => {
                if self.help_open {
                    self.help_open = false;
                } else if !self.gesture.is_none() {
                    self.cancel_gesture();
                } else if self.text_edit.is_some() {
                    self.cancel_text();
                } else if self.bookmarks.is_some() {
                    self.bookmarks = None;
                } else if self.find_open {
                    self.close_find();
                } else if self.tool != Tool::Nav {
                    self.set_tool(Tool::Nav);
                }
            }
        }
    }

    // --- frame plumbing ------------------------------------------------------------

    /// Open a PDF dropped onto the window.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let pdf = dropped.iter().filter_map(|f| f.path.clone()).find(|p| {
            p.extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
        });
        match pdf {
            Some(path) => self.open_path(&path),
            None => self.error("Only PDF files can be opened"),
        }
    }

    /// Save on close; if saving fails, refuse the first close so annotations aren't lost
    /// without warning.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // Keep whatever is in progress: a stroke whose release lands in the same frame as
        // the close request must not be lost.
        self.commit_text();
        self.finish_gesture(true);
        if self.close_armed || !self.dirty {
            return;
        }
        if let Err(e) = self.save_now() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_armed = true;
            self.error(format!(
                "Couldn't save annotations ({e}) — close again to quit without them"
            ));
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.pdf_path {
            Some(_) => format!("{} — nullpdf", self.file_name()),
            None => "nullpdf".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for App {
    /// Tab toggles the chrome; it is removed from egui's input here so it can never move
    /// keyboard focus onto a toolbar widget (which would swallow later shortcuts).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let typing = ctx.text_edit_focused();
        raw_input.events.retain(|e| match e {
            egui::Event::Key {
                key: egui::Key::Tab,
                pressed,
                modifiers,
                ..
            } => {
                if *pressed && !self.tab_down && !typing && !modifiers.any() {
                    self.queued.push(Action::ToggleChrome);
                }
                self.tab_down = *pressed;
                false
            }
            _ => true,
        });
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.05);

        self.check_reload();
        self.handle_drops(&ctx);
        self.handle_close(&ctx);
        self.handle_keyboard();

        let mut pending_render = false;
        egui::CentralPanel::no_frame().show_inside(root, |ui| {
            let (response, painter) =
                ui.allocate_painter(ui.available_size(), Sense::CLICK | Sense::DRAG);
            self.viewport = response.rect;
            if self.needs_fit && !self.layout.is_empty() {
                self.initial_view();
            }
            self.keep_fit();
            self.handle_pointer(&response);
            self.update_camera(dt);
            pending_render = self.prepare_pages(&ctx);
            self.paint(&painter, &response);
        });
        self.text_overlay();

        let mut actions = Vec::new();
        self.chrome(&mut actions);
        for a in actions {
            self.dispatch(a);
        }

        let searching = self.search_tick();
        self.autosave_tick();
        self.update_title(&ctx);

        // Repaint scheduling: animations, pending renders and searches now; settling,
        // autosave and a failed reload a little later. Otherwise the app sleeps until input
        // (or the file watcher) wakes it.
        if pending_render || searching || self.glide.is_some() {
            ctx.request_repaint();
        }
        if let Some(t) = &self.toast {
            // Frames only while fading in/out; in between, wake up when the fade-out starts.
            let age = self.now - t.born;
            let fade_out = t.ttl() - TOAST_FADE_OUT;
            if age < TOAST_FADE_IN || age >= fade_out {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(fade_out - age));
            }
        }
        if self.now - self.view_changed_at <= SETTLE_TIME + 0.05 {
            ctx.request_repaint_after(Duration::from_secs_f64(SETTLE_TIME));
        }
        if self.dirty {
            ctx.request_repaint_after(Duration::from_secs_f64(AUTOSAVE_DELAY));
        }
        if self.reload_retry {
            ctx.request_repaint_after(RELOAD_POLL);
        }
    }

    fn on_exit(&mut self) {
        self.commit_text();
        self.finish_gesture(true);
        if self.dirty {
            let _ = self.save_now();
        }
    }
}

/// `c` with its alpha replaced (straight, not premultiplied).
fn with_alpha(c: Color32, a: u8) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// A size for display: "0.5", "1.5", "14".
fn format_size(v: f32) -> String {
    if (v - v.round()).abs() < 0.01 {
        format!("{}", v.round() as i32)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Last-modified time of `path`, or `None` if it can't be stat'd.
fn current_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Whether two paths name the same file (resolving symlinks/relative parts when possible).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
