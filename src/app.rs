//! The eframe application: wires the PDF renderer, camera, annotations and tools
//! together, handles input, and paints the rotated page canvas + strokes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use egui::{Color32, Pos2, Rect, Sense, Stroke as EStroke};
use pdfium_render::prelude::Pdfium;

use crate::annotation::{
    chaikin, Annotation, AnnotationSet, History, Stroke, StrokeKind, TextAnnotation,
};
use crate::pdf::PdfDoc;
use crate::tools::{ActiveStroke, Mode, TextDrag, TextEditing, Tools};
use crate::view::{Camera, Layout, PageSize};

/// Extra erase pick-radius beyond the stroke's own half-width, in doc points.
const ERASE_SLOP: f32 = 4.0;

/// Minimum movement (in normalised page units) before a new freehand sample is kept.
const MIN_SAMPLE_DIST: f32 = 0.0012;

/// How often to poll the open PDF's mtime for external changes.
const RELOAD_POLL: Duration = Duration::from_millis(750);

/// Multiplier applied to raw wheel scroll for a slightly brisker feel.
const SCROLL_SPEED: f32 = 1.4;

/// Maximum character width of a path shown in the status bar before middle-eliding.
const STATUS_PATH_MAX: usize = 48;

pub struct App {
    /// Leaked, app-lifetime Pdfium binding. `None` if the system library failed to load.
    pdfium: Option<&'static Pdfium>,
    pdf: Option<PdfDoc>,
    pdf_path: Option<PathBuf>,
    /// Last-modified time of `pdf_path` when last loaded, for change detection.
    pdf_mtime: Option<SystemTime>,
    layout: Layout,
    set: AnnotationSet,
    history: History,
    cam: Camera,
    tools: Tools,

    /// Current pen colour (straight sRGBA); alpha distinguishes ink from highlight.
    color: Color32,
    width_pt: f32,
    /// Current text font size in PDF points (Text mode).
    font_pt: f32,

    status: String,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let (pdfium, status) = match PdfDoc::init_pdfium() {
            Ok(p) => (Some(p), String::from("Open a PDF (Ctrl+O).")),
            Err(e) => (None, format!("Failed to load libpdfium: {e}")),
        };

        let mut app = Self {
            pdfium,
            pdf: None,
            pdf_path: None,
            pdf_mtime: None,
            layout: Layout::default(),
            set: AnnotationSet::default(),
            history: History::default(),
            cam: Camera::default(),
            tools: Tools::default(),
            color: Color32::from_rgb(220, 40, 40),
            width_pt: 3.0,
            font_pt: 14.0,
            status,
        };

        if let Some(path) = initial {
            app.open_path(&path);
        }
        app
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
        {
            self.open_path(&path);
        }
    }

    fn open_path(&mut self, path: &Path) {
        let Some(pdfium) = self.pdfium else {
            self.status = "pdfium unavailable".into();
            return;
        };
        match PdfDoc::open(pdfium, path) {
            Ok(doc) => {
                let sizes: Vec<PageSize> = doc.sizes.clone();
                let page_count = sizes.len();
                self.layout = Layout::new(sizes);
                self.pdf = Some(doc);
                self.pdf_path = Some(path.to_path_buf());

                let file_name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Reload sidecar annotations if present and shaped for this document.
                self.set = match AnnotationSet::load_for(path) {
                    Some(s) if s.page_count == page_count => {
                        self.status = format!("{file_name} — loaded annotations");
                        s
                    }
                    Some(mut s) => {
                        // Page count changed: keep strokes but warn (best-effort alignment).
                        s.pdf_file = file_name.clone();
                        self.status =
                            format!("{file_name} — page count changed, annotations may shift");
                        s
                    }
                    None => {
                        self.status = format!("{file_name} — {page_count} pages");
                        AnnotationSet::new(file_name, page_count)
                    }
                };
                self.history = History::default();
                self.cam.initialized = false;
                self.tools.active = None;
                self.pdf_mtime = current_mtime(path);
            }
            Err(e) => {
                self.status = format!("Failed to open: {e}");
            }
        }
    }

    /// Re-open the PDF (pages only) when the file changes on disk, preserving the
    /// in-memory annotations (normalised, so they re-align) and the current view.
    /// Skipped while a stroke is in progress to avoid disrupting drawing.
    fn check_reload(&mut self) {
        if self.tools.active.is_some() {
            return;
        }
        let (Some(pdfium), Some(path)) = (self.pdfium, self.pdf_path.clone()) else {
            return;
        };
        let Some(new_mtime) = current_mtime(&path) else {
            return;
        };
        if self.pdf_mtime == Some(new_mtime) {
            return;
        }
        // File changed (or appeared): try to re-open. A partial write will fail here;
        // we leave mtime untouched so the next tick retries once the writer finishes.
        match PdfDoc::open(pdfium, &path) {
            Ok(doc) => {
                let sizes = doc.sizes.clone();
                let page_count = sizes.len();
                self.layout = Layout::new(sizes);
                self.pdf = Some(doc);
                self.pdf_mtime = Some(new_mtime);
                if page_count != self.set.page_count {
                    self.set.page_count = page_count;
                    self.status = format!(
                        "Auto-reloaded — page count now {page_count}, annotations may shift"
                    );
                } else {
                    self.status = "Auto-reloaded (file changed on disk)".into();
                }
            }
            Err(_) => { /* mid-write or transiently unreadable; retry next tick */ }
        }
    }

    fn export_dialog(&mut self) {
        let (Some(pdfium), Some(src)) = (self.pdfium, self.pdf_path.clone()) else {
            return;
        };
        let default_name = src
            .file_stem()
            .map(|s| format!("{}-annotated.pdf", s.to_string_lossy()))
            .unwrap_or_else(|| "annotated.pdf".to_string());
        if let Some(out) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(default_name)
            .save_file()
        {
            match PdfDoc::export_flattened(pdfium, &src, &out, &self.set.strokes, &self.set.texts) {
                Ok(()) => self.status = format!("Exported flattened PDF → {}", elide_path(&out)),
                Err(e) => self.status = format!("Export failed: {e}"),
            }
        }
    }

    fn save_sidecar(&mut self) {
        if let Some(path) = self.pdf_path.clone() {
            match self.set.save_for(&path) {
                Ok(()) => self.status = "Saved annotations".into(),
                Err(e) => self.status = format!("Save failed: {e}"),
            }
        }
    }

    /// Build a finished stroke from the active gesture and current pen settings.
    fn finalize_stroke(&mut self, straight: bool) {
        let Some(active) = self.tools.active.take() else {
            return;
        };
        if active.points.len() < 2 {
            return;
        }
        let (kind, points) = if straight {
            (
                StrokeKind::Line,
                vec![active.points[0], *active.points.last().unwrap()],
            )
        } else {
            // Smooth freehand strokes (Chaikin) so committed ink matches the preview.
            (StrokeKind::Freehand, chaikin(&active.points, 2))
        };
        let stroke = Stroke {
            page: active.page,
            kind,
            points,
            color: self.color.to_srgba_unmultiplied(),
            width_pt: self.width_pt,
            id: 0,
        };
        let id = self.set.add(stroke);
        // Record a clone carrying the assigned id for undo.
        if let Some(s) = self.set.strokes.iter().find(|s| s.id == id) {
            self.history.record_add(Annotation::Stroke(s.clone()));
        }
    }

    /// Remove the topmost annotation (text first, then stroke) under `doc`.
    fn erase_at(&mut self, ctx: &egui::Context, doc: Pos2) {
        // Text sits visually on top of strokes, so prefer a text hit.
        let mut hit = self.text_at_doc(ctx, doc);
        if hit.is_none() {
            for s in self.set.strokes.iter().rev() {
                let thresh = s.width_pt * 0.5 + ERASE_SLOP;
                if self.stroke_hit(s, doc, thresh) {
                    hit = Some(s.id);
                    break;
                }
            }
        }
        if let Some(id) = hit {
            if let Some(removed) = self.set.remove(id) {
                self.history.record_erase(removed);
            }
        }
    }

    /// True if `doc` lies within `thresh` points of any segment of stroke `s`.
    fn stroke_hit(&self, s: &Stroke, doc: Pos2, thresh: f32) -> bool {
        if s.points.is_empty() {
            return false;
        }
        let pts: Vec<Pos2> = s
            .points
            .iter()
            .map(|n| self.layout.norm_to_doc(s.page, *n))
            .collect();
        if pts.len() == 1 {
            return pts[0].distance(doc) <= thresh;
        }
        pts.windows(2)
            .any(|w| point_segment_distance(doc, w[0], w[1]) <= thresh)
    }

    // --- text ------------------------------------------------------------------

    /// Axis-aligned bounding box of a text annotation in doc space. Text is laid out at
    /// its point size, so the galley's size in egui points equals its extent in PDF points.
    fn text_bbox_doc(&self, ctx: &egui::Context, t: &TextAnnotation) -> Rect {
        let anchor = self.layout.norm_to_doc(t.page, t.pos);
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                t.text.clone(),
                egui::FontId::proportional(t.size_pt.max(1.0)),
                Color32::WHITE,
            )
        });
        Rect::from_min_size(anchor, galley.size())
    }

    /// Id of the topmost text annotation whose box contains doc point `doc`, if any.
    fn text_at_doc(&self, ctx: &egui::Context, doc: Pos2) -> Option<u64> {
        self.set
            .texts
            .iter()
            .rev()
            .find(|t| self.text_bbox_doc(ctx, t).contains(doc))
            .map(|t| t.id)
    }

    fn handle_text(&mut self, ctx: &egui::Context, response: &egui::Response, viewport: Rect) {
        // Finish editing before starting any new gesture (a click outside the editor
        // commits it via lost-focus in `text_overlay`); ignore canvas input meanwhile.
        if self.tools.text_edit.is_some() {
            return;
        }
        let primary = egui::PointerButton::Primary;

        // Begin dragging an existing text.
        if response.drag_started_by(primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                let doc = self.cam.screen_to_doc(pos, viewport);
                if let Some(id) = self.text_at_doc(ctx, doc) {
                    if let Some(t) = self.set.text_by_id(id) {
                        let anchor = self.layout.norm_to_doc(t.page, t.pos);
                        self.tools.text_drag = Some(TextDrag {
                            id,
                            before: t.clone(),
                            grab: doc - anchor,
                        });
                    }
                }
            }
        }

        // Continue a drag, keeping the grab point under the cursor.
        if response.dragged_by(primary) {
            if let Some(drag) = self.tools.text_drag.clone() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let doc = self.cam.screen_to_doc(pos, viewport);
                    let new_anchor = doc - drag.grab;
                    let r = self.layout.page_rect(drag.before.page);
                    let nx = ((new_anchor.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                    let ny = ((new_anchor.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                    self.set.set_text_pos(drag.id, [nx, ny]);
                }
            }
        }

        // Finish a drag: record one undo entry only if it actually moved.
        if response.drag_stopped_by(primary) {
            if let Some(drag) = self.tools.text_drag.take() {
                if let Some(after) = self.set.text_by_id(drag.id).cloned() {
                    if after.pos != drag.before.pos {
                        self.history.record_edit_text(drag.before, after);
                    }
                }
            }
        }

        // A plain click edits the text under the cursor, or opens a new editor.
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let doc = self.cam.screen_to_doc(pos, viewport);
                if let Some(id) = self.text_at_doc(ctx, doc) {
                    self.open_existing_text(id);
                } else if let Some((page, norm)) = self.layout.page_at_doc(doc) {
                    self.open_new_text(page, norm);
                }
            }
        }
    }

    fn open_new_text(&mut self, page: usize, pos: [f32; 2]) {
        self.tools.text_edit = Some(TextEditing {
            page,
            pos,
            buffer: String::new(),
            editing_id: None,
            just_opened: true,
        });
    }

    /// Open the editor on an existing annotation, loading its style into the pen so the
    /// toolbar reflects it (and edits to colour/size apply on commit).
    fn open_existing_text(&mut self, id: u64) {
        if let Some(t) = self.set.text_by_id(id) {
            self.color =
                Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], t.color[3]);
            self.font_pt = t.size_pt;
            self.tools.text_edit = Some(TextEditing {
                page: t.page,
                pos: t.pos,
                buffer: t.text.clone(),
                editing_id: Some(id),
                just_opened: true,
            });
        }
    }

    /// Commit the open editor: add a new text, update an existing one (content/colour/size),
    /// or delete it if emptied. Discards a new-but-empty editor.
    fn commit_text(&mut self) {
        let Some(te) = self.tools.text_edit.take() else {
            return;
        };
        let text = te.buffer.trim().to_string();
        let color = self.color.to_srgba_unmultiplied();

        match te.editing_id {
            Some(id) => {
                let Some(before) = self.set.text_by_id(id).cloned() else {
                    return;
                };
                if text.is_empty() {
                    if let Some(removed) = self.set.remove(id) {
                        self.history.record_erase(removed);
                    }
                    return;
                }
                let after = TextAnnotation {
                    text,
                    color,
                    size_pt: self.font_pt,
                    ..before.clone()
                };
                if after.text != before.text
                    || after.color != before.color
                    || after.size_pt != before.size_pt
                {
                    self.set.replace_text(after.clone());
                    self.history.record_edit_text(before, after);
                }
            }
            None => {
                if text.is_empty() {
                    return;
                }
                let t = TextAnnotation {
                    page: te.page,
                    pos: te.pos,
                    text,
                    color,
                    size_pt: self.font_pt,
                    id: 0,
                };
                let id = self.set.add_text(t);
                if let Some(added) = self.set.text_by_id(id) {
                    self.history.record_add(Annotation::Text(added.clone()));
                }
            }
        }
    }

    fn cancel_text(&mut self) {
        self.tools.text_edit = None;
    }

    /// Render the live text editor (a small overlay anchored at the text position).
    /// Enter or clicking away commits; Escape cancels. The editor itself stays
    /// axis-aligned even when the page is rotated; committed text follows the rotation.
    fn text_overlay(&mut self, ctx: &egui::Context, viewport: Rect) {
        let Some(te) = self.tools.text_edit.as_ref() else {
            return;
        };
        let anchor = self
            .cam
            .doc_to_screen(self.layout.norm_to_doc(te.page, te.pos), viewport);
        let px = (self.font_pt * self.cam.zoom).max(8.0);

        let (mut commit, mut cancel) = (false, false);
        egui::Area::new(egui::Id::new("nullpdf_text_editor"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                let te = self.tools.text_edit.as_mut().unwrap();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut te.buffer)
                        .font(egui::FontId::proportional(px))
                        .text_color(self.color)
                        .desired_width((px * 8.0).max(120.0)),
                );
                if te.just_opened {
                    resp.request_focus();
                    te.just_opened = false;
                }
                if resp.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    } else {
                        commit = true;
                    }
                }
            });
        if commit {
            self.commit_text();
        } else if cancel {
            self.cancel_text();
        }
    }

    // --- input -----------------------------------------------------------------

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (mut want_open, mut want_save, mut want_export) = (false, false, false);
        // While a text editor is focused, single-letter keys are text input, not shortcuts.
        let typing = ctx.egui_wants_keyboard_input();
        ctx.input(|i| {
            if !typing && i.key_pressed(egui::Key::D) {
                self.tools.mode = Mode::Draw;
            }
            if !typing && i.key_pressed(egui::Key::T) {
                self.tools.mode = Mode::Text;
            }
            if !typing && i.key_pressed(egui::Key::Q) {
                self.tools.mode = Mode::Nav;
                self.tools.active = None;
            }
            let cmd = i.modifiers.command;
            if !typing && i.key_pressed(egui::Key::E) && !cmd && self.tools.mode == Mode::Draw {
                self.tools.eraser = !self.tools.eraser;
            }
            want_open = cmd && i.key_pressed(egui::Key::O);
            want_save = cmd && i.key_pressed(egui::Key::S);
            want_export = cmd && i.key_pressed(egui::Key::E);
            if cmd && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift {
                    self.history.redo(&mut self.set);
                } else {
                    self.history.undo(&mut self.set);
                }
            }
            if cmd && i.key_pressed(egui::Key::Y) {
                self.history.redo(&mut self.set);
            }
        });
        // rfd / save need `&mut self`, which isn't available inside the input closure.
        if want_open {
            self.open_dialog();
        }
        if want_save {
            self.save_sidecar();
        }
        if want_export {
            self.export_dialog();
        }
    }

    /// Wheel scroll / ctrl-wheel zoom, applied when the pointer is over the canvas.
    fn handle_scroll_zoom(&mut self, ctx: &egui::Context, viewport: Rect) {
        ctx.input(|i| {
            let Some(cursor) = i.pointer.hover_pos() else {
                return;
            };
            if !viewport.contains(cursor) {
                return;
            }
            let zoom_delta = i.zoom_delta();
            if (zoom_delta - 1.0).abs() > f32::EPSILON {
                self.cam.zoom_around(zoom_delta, cursor, viewport);
            } else {
                let dy = i.smooth_scroll_delta.y;
                if dy != 0.0 {
                    // Wheel up moves up the page stack; negate so the gesture isn't
                    // inverted, and scale for a slightly brisker feel.
                    self.cam.scroll(-dy * SCROLL_SPEED);
                }
            }
        });
    }

    fn handle_canvas(&mut self, ctx: &egui::Context, response: &egui::Response, viewport: Rect) {
        // Right-drag rotates in either mode.
        if response.dragged_by(egui::PointerButton::Secondary) {
            if let Some(pos) = response.interact_pointer_pos() {
                let cur = (pos - viewport.center()).angle();
                if let Some(last) = self.tools.last_rotate_angle {
                    let mut d = cur - last;
                    let pi = std::f32::consts::PI;
                    while d > pi {
                        d -= std::f32::consts::TAU;
                    }
                    while d < -pi {
                        d += std::f32::consts::TAU;
                    }
                    self.cam.rotate_by(d);
                }
                self.tools.last_rotate_angle = Some(cur);
            }
        } else {
            self.tools.last_rotate_angle = None;
        }

        match self.tools.mode {
            Mode::Nav => {
                if response.dragged_by(egui::PointerButton::Primary) {
                    self.cam.pan(response.drag_delta());
                }
            }
            Mode::Draw => self.handle_draw(ctx, response, viewport),
            Mode::Text => self.handle_text(ctx, response, viewport),
        }
    }

    fn handle_draw(&mut self, ctx: &egui::Context, response: &egui::Response, viewport: Rect) {
        let shift = ctx.input(|i| i.modifiers.shift);
        let primary = egui::PointerButton::Primary;

        if response.drag_started_by(primary) || response.dragged_by(primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                let doc = self.cam.screen_to_doc(pos, viewport);
                if self.tools.eraser {
                    self.erase_at(ctx, doc);
                    return;
                }
                match &mut self.tools.active {
                    None => {
                        if let Some((page, norm)) = self.layout.page_at_doc(doc) {
                            self.tools.active = Some(ActiveStroke {
                                page,
                                points: vec![norm],
                            });
                        }
                    }
                    Some(active) => {
                        // Lock subsequent points to the page the stroke started on.
                        let r = self.layout.page_rect(active.page);
                        let nx = ((doc.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                        let ny = ((doc.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                        // Drop near-duplicate samples so jitter doesn't accumulate; the
                        // Chaikin pass on finalize then smooths what remains.
                        let last = active.points.last().copied().unwrap_or([nx, ny]);
                        let moved = (nx - last[0]).hypot(ny - last[1]);
                        if moved >= MIN_SAMPLE_DIST {
                            active.points.push([nx, ny]);
                        }
                    }
                }
            }
        }

        if response.drag_stopped_by(primary) && !self.tools.eraser {
            self.finalize_stroke(shift);
        }
    }

    // --- painting --------------------------------------------------------------

    fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter, viewport: Rect) {
        // Pages as rotated textured quads.
        for i in 0..self.layout.pages.len() {
            let r = self.layout.page_rect(i);
            let corners_doc = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
            let scr: [Pos2; 4] =
                std::array::from_fn(|k| self.cam.doc_to_screen(corners_doc[k], viewport));
            let bb = Rect::from_points(&scr);
            if !bb.intersects(viewport) {
                continue;
            }

            if let Some(pdf) = self.pdf.as_mut() {
                if let Some((tex_id, _)) = pdf.page_texture(ctx, i, self.cam.zoom) {
                    let uv = [
                        Pos2::new(0.0, 0.0),
                        Pos2::new(1.0, 0.0),
                        Pos2::new(1.0, 1.0),
                        Pos2::new(0.0, 1.0),
                    ];
                    let mut mesh = egui::Mesh::with_texture(tex_id);
                    for k in 0..4 {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: scr[k],
                            uv: uv[k],
                            color: Color32::WHITE,
                        });
                    }
                    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                    painter.add(mesh);
                    continue;
                }
            }
            // Fallback placeholder while rendering / when no doc.
            painter.add(egui::Shape::convex_polygon(
                scr.to_vec(),
                Color32::from_gray(48),
                EStroke::NONE,
            ));
        }

        // Committed strokes.
        for s in &self.set.strokes {
            self.paint_stroke(painter, viewport, &s.points, s.page, s.color, s.width_pt);
        }

        // Committed text, except the one currently shown by the live editor.
        let editing = self.tools.text_edit.as_ref().and_then(|te| te.editing_id);
        for t in &self.set.texts {
            if Some(t.id) == editing {
                continue;
            }
            self.paint_text(ctx, painter, viewport, t);
        }

        // In-progress preview using current pen settings.
        if let Some(active) = &self.tools.active {
            let shift = ctx.input(|i| i.modifiers.shift);
            let color = self.color.to_srgba_unmultiplied();
            if shift && active.points.len() >= 2 {
                let pts = [active.points[0], *active.points.last().unwrap()];
                self.paint_stroke(painter, viewport, &pts, active.page, color, self.width_pt);
            } else {
                // Preview the smoothed path so it matches the committed stroke (WYSIWYG).
                let smoothed = chaikin(&active.points, 2);
                self.paint_stroke(painter, viewport, &smoothed, active.page, color, self.width_pt);
            }
        }
    }

    fn paint_stroke(
        &self,
        painter: &egui::Painter,
        viewport: Rect,
        norm_points: &[[f32; 2]],
        page: usize,
        color: [u8; 4],
        width_pt: f32,
    ) {
        if norm_points.len() < 2 {
            return;
        }
        let pts: Vec<Pos2> = norm_points
            .iter()
            .map(|n| {
                let doc = self.layout.norm_to_doc(page, *n);
                self.cam.doc_to_screen(doc, viewport)
            })
            .collect();
        let col = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
        let stroke = EStroke::new((width_pt * self.cam.zoom).max(0.5), col);
        painter.add(egui::Shape::line(pts, stroke));
    }

    /// Paint a committed text annotation as a rotated, zoom-scaled galley.
    fn paint_text(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        viewport: Rect,
        t: &TextAnnotation,
    ) {
        if t.text.is_empty() {
            return;
        }
        let px = (t.size_pt * self.cam.zoom).max(1.0);
        let col = Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], t.color[3]);
        let galley =
            ctx.fonts_mut(|f| f.layout_no_wrap(t.text.clone(), egui::FontId::proportional(px), col));
        let anchor = self
            .cam
            .doc_to_screen(self.layout.norm_to_doc(t.page, t.pos), viewport);
        painter.add(egui::epaint::TextShape::new(anchor, galley, col).with_angle(self.cam.rotation));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open").clicked() {
                self.open_dialog();
            }
            ui.add_enabled_ui(self.pdf.is_some(), |ui| {
                if ui.button("Export").clicked() {
                    self.export_dialog();
                }
            });
            ui.separator();

            ui.selectable_value(&mut self.tools.mode, Mode::Nav, "Nav (Q)");
            ui.selectable_value(&mut self.tools.mode, Mode::Draw, "Draw (D)");
            ui.selectable_value(&mut self.tools.mode, Mode::Text, "Text (T)");

            ui.add_enabled_ui(self.tools.mode == Mode::Draw, |ui| {
                let label = ui.selectable_label(self.tools.eraser, "Eraser (E)");
                if label.clicked() {
                    self.tools.eraser = !self.tools.eraser;
                }
            });

            ui.separator();
            egui::color_picker::color_edit_button_srgba(
                ui,
                &mut self.color,
                egui::color_picker::Alpha::OnlyBlend,
            );
            // The width slider doubles as the font-size slider in Text mode.
            if self.tools.mode == Mode::Text {
                ui.add(
                    egui::Slider::new(&mut self.font_pt, 6.0..=96.0)
                        .text("font")
                        .fixed_decimals(0),
                );
            } else {
                ui.add(
                    egui::Slider::new(&mut self.width_pt, 0.5..=24.0)
                        .text("width")
                        .fixed_decimals(1),
                );
            }

            ui.separator();
            ui.add_enabled_ui(self.history.can_undo(), |ui| {
                if ui.button("Undo").clicked() {
                    self.history.undo(&mut self.set);
                }
            });
            ui.add_enabled_ui(self.history.can_redo(), |ui| {
                if ui.button("Redo").clicked() {
                    self.history.redo(&mut self.set);
                }
            });

            if ui.button("Reset rotation").clicked() {
                self.cam.rotation = 0.0;
            }
        });
    }

    /// Bottom status bar: a single truncating line so long messages/paths stay on
    /// one row instead of wrapping or pushing the layout.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(&self.status).truncate());
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        // Watch the source file for external edits and keep waking up to poll it.
        self.check_reload();
        if self.pdf_path.is_some() {
            ctx.request_repaint_after(RELOAD_POLL);
        }

        self.handle_keyboard(&ctx);

        egui::Panel::top("toolbar").show_inside(root, |ui| {
            self.toolbar(ui);
        });

        egui::Panel::bottom("statusbar").show_inside(root, |ui| {
            self.status_bar(ui);
        });

        egui::CentralPanel::default().show_inside(root, |ui| {
            let (response, painter) =
                ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
            let viewport = response.rect;

            if !self.cam.initialized && !self.layout.pages.is_empty() {
                self.cam.fit(&self.layout, viewport);
            }

            self.handle_scroll_zoom(&ctx, viewport);
            self.handle_canvas(&ctx, &response, viewport);
            if !self.layout.pages.is_empty() {
                self.cam.clamp_to(&self.layout, viewport);
            }
            self.paint(&ctx, &painter, viewport);
            self.text_overlay(&ctx, viewport);

            // Cursor hint by mode.
            let cursor = match (self.tools.mode, self.tools.eraser) {
                (Mode::Nav, _) => egui::CursorIcon::Grab,
                (Mode::Draw, true) => egui::CursorIcon::NotAllowed,
                (Mode::Draw, false) => egui::CursorIcon::Crosshair,
                (Mode::Text, _) => egui::CursorIcon::Text,
            };
            ui.ctx().set_cursor_icon(cursor);
        });
    }
}

/// Shorten a path for the status bar, keeping the tail (filename) visible and
/// middle-eliding with `…` so long paths don't overflow the bar.
fn elide_path(path: &Path) -> String {
    let full = path.display().to_string();
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= STATUS_PATH_MAX {
        return full;
    }
    // Reserve a few leading chars and keep as much of the tail as fits.
    let head = 6;
    let tail = STATUS_PATH_MAX.saturating_sub(head + 1);
    let head_str: String = chars[..head].iter().collect();
    let tail_str: String = chars[chars.len() - tail..].iter().collect();
    format!("{head_str}…{tail_str}")
}

/// Last-modified time of `path`, or `None` if it can't be stat'd.
fn current_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Shortest distance from point `p` to segment `a`–`b`.
fn point_segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    let proj = a + ab * t;
    p.distance(proj)
}
