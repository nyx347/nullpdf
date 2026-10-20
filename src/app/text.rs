//! Text annotations: layout and hit-testing, the in-place editor, and the glyph runs
//! the exporter burns in.

use std::borrow::Cow;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, FontFamily, FontId, Galley, Id, Key, KeyboardShortcut, Modifiers, Pos2, Rect};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;

use super::App;
use crate::annotation::{Annotation, TextAnnotation};
use crate::pdf::{FilledShape, PathCmd, TextRun};
use crate::theme;
use crate::tools::TextEditing;

/// Id of the editor's `TextEdit` (needed to pre-place the caret).
fn editor_id() -> Id {
    Id::new("nullpdf_text_editor")
}

impl App {
    /// Lay out `text` at `size` (egui points), as text annotations are drawn.
    fn galley(&self, text: &str, size: f32, color: Color32) -> Arc<Galley> {
        self.ctx.fonts_mut(|f| {
            f.layout_no_wrap(text.to_owned(), FontId::proportional(size.max(1.0)), color)
        })
    }

    /// Axis-aligned bounding box of a text annotation in doc space. Text is laid out at
    /// its point size, so the galley's size in egui points equals its extent in PDF points.
    pub(super) fn text_bbox_doc(&self, t: &TextAnnotation) -> Rect {
        let anchor = self.layout.norm_to_doc(t.page, t.pos);
        Rect::from_min_size(
            anchor,
            self.galley(&t.text, t.size_pt, Color32::WHITE).size(),
        )
    }

    /// Id of the topmost text annotation whose box contains doc point `doc`, if any.
    pub(super) fn text_at_doc(&self, doc: Pos2) -> Option<u64> {
        self.set
            .texts
            .iter()
            .rev()
            .filter(|t| t.page < self.layout.len())
            .find(|t| {
                self.text_bbox_doc(t)
                    .expand(2.0 / self.cam.zoom)
                    .contains(doc)
            })
            .map(|t| t.id)
    }

    /// Open an editor for new text on `page`, with its first line centred on the click (the
    /// I-beam's hotspot) rather than hanging below it.
    pub(super) fn open_new_text(&mut self, page: usize, click: Pos2) {
        let row = self.galley("X", self.text_size, Color32::WHITE).size().y;
        let anchor = click - egui::vec2(0.0, row * 0.5);
        let n = self.layout.doc_to_norm(page, anchor);
        self.text_edit = Some(TextEditing {
            page,
            pos: [n[0].clamp(0.0, 1.0), n[1].clamp(0.0, 1.0)],
            buffer: String::new(),
            color: self.pen.color,
            size_pt: self.text_size,
            editing_id: None,
            just_opened: true,
            caret: None,
        });
    }

    /// Open the editor on an existing annotation, caret where it was clicked. Its style is
    /// edited in the editor; the pen's own colour and size are left alone.
    pub(super) fn open_existing_text(&mut self, id: u64, click: Option<Pos2>) {
        let Some(t) = self.set.text_by_id(id).cloned() else {
            return;
        };
        let caret = click.map(|doc| {
            let anchor = self.layout.norm_to_doc(t.page, t.pos);
            let galley = self.galley(&t.text, t.size_pt, Color32::WHITE);
            galley.cursor_from_pos(doc - anchor).index
        });
        self.text_edit = Some(TextEditing {
            page: t.page,
            pos: t.pos,
            buffer: t.text.clone(),
            color: Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], t.color[3]),
            size_pt: t.size_pt,
            editing_id: Some(id),
            just_opened: true,
            caret,
        });
    }

    /// Commit the open editor: add a new text, update an existing one (content/colour/size),
    /// or delete it if emptied. Discards a new-but-empty editor.
    pub(super) fn commit_text(&mut self) {
        let Some(te) = self.text_edit.take() else {
            return;
        };
        let text = te.buffer.trim_end().trim_start_matches('\n').to_string();
        let color = te.color.to_srgba_unmultiplied();
        match te.editing_id {
            Some(id) => {
                let Some(before) = self.set.text_by_id(id).cloned() else {
                    return;
                };
                if text.trim().is_empty() {
                    if let Some(removed) = self.set.remove(id) {
                        self.history.record_erase(vec![removed]);
                        self.mark_dirty();
                    }
                    return;
                }
                let after = TextAnnotation {
                    text,
                    color,
                    size_pt: te.size_pt,
                    ..before.clone()
                };
                if after != before {
                    self.set.replace_text(after.clone());
                    self.history.record_edit_text(before, after);
                    self.mark_dirty();
                }
            }
            None => {
                if text.trim().is_empty() {
                    return;
                }
                let id = self.set.add_text(TextAnnotation {
                    page: te.page,
                    pos: te.pos,
                    text,
                    color,
                    size_pt: te.size_pt,
                    id: 0,
                });
                if let Some(added) = self.set.text_by_id(id) {
                    self.history.record_add(Annotation::Text(added.clone()));
                }
                self.mark_dirty();
            }
        }
    }

    pub(super) fn cancel_text(&mut self) {
        self.text_edit = None;
    }

    /// The live editor: a frameless `TextEdit` laid exactly over where the committed text
    /// will be drawn (same font, size, colour and anchor), so typing is WYSIWYG. Enter or a
    /// click elsewhere commits, Shift+Enter starts a new line, Escape cancels. The editor
    /// stays axis-aligned when the page is rotated; committed text follows the rotation.
    pub(super) fn text_overlay(&mut self) {
        let ctx = self.ctx.clone();
        let Some(te) = self.text_edit.as_ref() else {
            return;
        };
        if te.page >= self.layout.len() {
            // The page vanished in an external reload.
            self.text_edit = None;
            return;
        }
        let anchor = self
            .cam
            .doc_to_screen(self.layout.norm_to_doc(te.page, te.pos), self.viewport);
        let px = (te.size_pt * self.cam.zoom).max(4.0);
        let color = te.color;
        let measure = if te.buffer.is_empty() {
            " "
        } else {
            &te.buffer
        };
        let size = self.galley(measure, px, color).size();
        // Leave room for the caret after the last character.
        let width = size.x + px * 0.5;

        let (mut commit, mut cancel) = (false, false);
        egui::Area::new(Id::new("nullpdf_text_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(&ctx, |ui| {
                let te = self.text_edit.as_mut().unwrap();
                if te.just_opened {
                    if let Some(index) = te.caret.take() {
                        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), editor_id())
                            .unwrap_or_default();
                        state
                            .cursor
                            .set_char_range(Some(CCursorRange::one(CCursor::new(index))));
                        state.store(ui.ctx(), editor_id());
                    }
                }
                // Lines break only where the user pressed Shift+Enter, exactly as the
                // committed annotation is laid out — never at the editor's width.
                let font = FontId::proportional(px);
                let mut no_wrap = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _: f32| {
                    ui.ctx().fonts_mut(|f| {
                        f.layout_no_wrap(buf.as_str().to_owned(), font.clone(), color)
                    })
                };
                let out = egui::TextEdit::multiline(&mut te.buffer)
                    .id(editor_id())
                    .font(FontId::proportional(px))
                    .text_color(color)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_rows(1)
                    .desired_width(width)
                    .clip_text(false)
                    .layouter(&mut no_wrap)
                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                    .show(ui);
                let resp = out.response.response;
                if te.just_opened {
                    resp.request_focus();
                    te.just_opened = false;
                }
                if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift)
                {
                    commit = true;
                }
                if resp.lost_focus() {
                    if ui.input(|i| i.key_pressed(Key::Escape)) {
                        cancel = true;
                    } else {
                        commit = true;
                    }
                }
                // A quiet frame around the text being edited.
                let frame = Rect::from_min_size(anchor, size).expand(3.0);
                ui.painter().rect_stroke(
                    frame,
                    3.0,
                    theme::line(1.0, theme::ACCENT.gamma_multiply(0.8)),
                    egui::StrokeKind::Outside,
                );
            });
        if commit {
            self.commit_text();
        } else if cancel {
            self.cancel_text();
        }
    }

    /// Text annotations as the exporter needs them: runs of real text in egui's primary
    /// proportional font (returned too, for embedding), and filled outlines for glyphs that
    /// come from its symbol/emoji fallback faces — so every glyph is exported in the face the
    /// screen draws it with. Positions come straight from egui's layout, so exported text
    /// sits exactly where it was drawn; runs are split per word to keep any kerning drift
    /// inside one word.
    pub(super) fn export_text(&self) -> (Vec<TextRun>, Vec<FilledShape>, Cow<'static, [u8]>) {
        let defs = egui::FontDefinitions::default();
        let faces: Vec<(Cow<'static, [u8]>, f32)> = defs
            .families
            .get(&FontFamily::Proportional)
            .into_iter()
            .flatten()
            .filter_map(|name| defs.font_data.get(name))
            .map(|d| (d.font.clone(), d.tweak.scale))
            .collect();
        let fonts: Vec<Option<skrifa::FontRef>> = faces
            .iter()
            .map(|(data, _)| skrifa::FontRef::new(data).ok())
            .collect();
        let glyph = |face: usize, c: char| fonts[face].as_ref()?.charmap().map(c);

        let mut runs = Vec::new();
        let mut shapes = Vec::new();
        for t in &self.set.texts {
            if t.page >= self.layout.len() || t.text.is_empty() {
                continue;
            }
            let p = self.layout.pages[t.page];
            let anchor = egui::vec2(t.pos[0] * p.w, t.pos[1] * p.h);
            let galley = self.galley(&t.text, t.size_pt, Color32::WHITE);
            for row in &galley.rows {
                let mut run: Option<TextRun> = None;
                for g in &row.row.glyphs {
                    let origin = anchor + row.pos.to_vec2() + g.pos.to_vec2();
                    let found = if g.chr.is_whitespace() {
                        None
                    } else {
                        (0..faces.len()).find_map(|f| Some((f, glyph(f, g.chr)?)))
                    };
                    match found {
                        Some((0, _)) => match run.as_mut() {
                            Some(r) if (r.baseline[1] - origin.y).abs() < 0.01 => {
                                r.text.push(g.chr)
                            }
                            _ => {
                                runs.extend(run.take());
                                run = Some(TextRun {
                                    page: t.page,
                                    text: g.chr.to_string(),
                                    baseline: [origin.x, origin.y],
                                    size_pt: t.size_pt,
                                    color: t.color,
                                });
                            }
                        },
                        Some((face, gid)) => {
                            runs.extend(run.take());
                            let Some(outline) = fonts[face]
                                .as_ref()
                                .and_then(|f| f.outline_glyphs().get(gid))
                            else {
                                continue;
                            };
                            let mut pen = OutlineToPath {
                                origin,
                                last: [0.0; 2],
                                path: Vec::new(),
                            };
                            let size = Size::new(t.size_pt * faces[face].1);
                            let settings = DrawSettings::unhinted(size, LocationRef::default());
                            if outline.draw(settings, &mut pen).is_ok() && !pen.path.is_empty() {
                                shapes.push(FilledShape {
                                    page: t.page,
                                    color: t.color,
                                    path: pen.path,
                                });
                            }
                        }
                        None => runs.extend(run.take()),
                    }
                }
                runs.extend(run);
            }
        }
        let primary = faces.into_iter().next().map(|(d, _)| d).unwrap_or_default();
        (runs, shapes, primary)
    }
}

/// Collects a glyph outline (font units scaled to points, y-up from the baseline) as a
/// path in page-local points (y-down), converting quadratic segments to cubics.
struct OutlineToPath {
    origin: egui::Vec2,
    last: [f32; 2],
    path: Vec<PathCmd>,
}

impl OutlineToPath {
    fn map(&self, x: f32, y: f32) -> [f32; 2] {
        [self.origin.x + x, self.origin.y - y]
    }
}

impl OutlinePen for OutlineToPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last = self.map(x, y);
        self.path.push(PathCmd::Move(self.last));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.last = self.map(x, y);
        self.path.push(PathCmd::Line(self.last));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (p0, q, p) = (self.last, self.map(cx0, cy0), self.map(x, y));
        let lerp = |a: [f32; 2], b: [f32; 2]| {
            [
                a[0] + (b[0] - a[0]) * 2.0 / 3.0,
                a[1] + (b[1] - a[1]) * 2.0 / 3.0,
            ]
        };
        self.path.push(PathCmd::Cubic(lerp(p0, q), lerp(p, q), p));
        self.last = p;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.last = self.map(x, y);
        self.path.push(PathCmd::Cubic(
            self.map(cx0, cy0),
            self.map(cx1, cy1),
            self.last,
        ));
    }

    fn close(&mut self) {
        self.path.push(PathCmd::Close);
    }
}
