//! Painting the document: page shadows and textures, search matches, strokes, text, the
//! live stroke preview, hover feedback, custom cursors and the rotation readout.

use std::ops::Range;

use egui::epaint::{Mesh, TextShape, Vertex};
use egui::{
    Color32, CursorIcon, FontId, Painter, Pos2, Rect, Response, Shape, Stroke, StrokeKind, Vec2,
};

use super::App;
use crate::annotation::chaikin;
use crate::pdf::PageImage;
use crate::theme;
use crate::tools::{Gesture, Tool};

/// Soft shadow under each page: blur radius and downward offset (screen px).
const SHADOW_BLUR: f32 = 12.0;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_ALPHA: u8 = 90;

impl App {
    pub(super) fn paint(&self, painter: &Painter, response: &Response) {
        let viewport = self.viewport;
        painter.rect_filled(viewport, 0.0, theme::CANVAS);
        if self.layout.is_empty() {
            return;
        }
        let ppp = painter.pixels_per_point();
        let view = self.cam.doc_bbox(viewport, viewport);
        let visible = self.layout.pages_in(view);

        // All shadows first, so no shadow ever falls across a neighbouring page.
        for i in visible.clone() {
            self.paint_shadow(painter, i);
        }
        for i in visible.clone() {
            self.paint_page(painter, i, ppp);
        }
        self.paint_search_hits(painter, visible.clone());

        for s in &self.set.strokes {
            if visible.contains(&s.page) {
                self.paint_stroke(painter, &s.points, s.page, s.color, s.width_pt);
            }
        }
        let editing = self.text_edit.as_ref().and_then(|te| te.editing_id);
        for t in &self.set.texts {
            if visible.contains(&t.page) && Some(t.id) != editing {
                self.paint_text(painter, t);
            }
        }

        // In-progress stroke, drawn exactly as it will be committed (WYSIWYG).
        if let Gesture::Draw(active) = &self.gesture {
            let brush = if self.tool == Tool::Highlighter {
                self.highlighter
            } else {
                self.pen
            };
            let color = brush.color.to_srgba_unmultiplied();
            let first = active.points[0];
            let last = *active.points.last().unwrap();
            if self.mods.shift && active.points.len() >= 2 {
                let end = self.straight_end(active.page, first, last);
                self.paint_stroke(painter, &[first, end], active.page, color, brush.size);
            } else {
                let smooth = chaikin(&active.points, 2);
                self.paint_stroke(painter, &smooth, active.page, color, brush.size);
            }
        }

        self.paint_feedback(painter, response);
    }

    /// The four screen-space corners of page `i` (top-left, top-right, bottom-right,
    /// bottom-left).
    fn page_corners(&self, i: usize) -> [Pos2; 4] {
        let r = self.layout.page_rect(i);
        [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ]
        .map(|p| self.cam.doc_to_screen(p, self.viewport))
    }

    fn paint_shadow(&self, painter: &Painter, i: usize) {
        let c = self
            .page_corners(i)
            .map(|p| p + Vec2::new(0.0, SHADOW_OFFSET));
        let ax = (c[1] - c[0]).normalized() * SHADOW_BLUR;
        let ay = (c[3] - c[0]).normalized() * SHADOW_BLUR;
        let outer = [
            c[0] - ax - ay,
            c[1] + ax - ay,
            c[2] + ax + ay,
            c[3] - ax + ay,
        ];
        let dark = Color32::from_black_alpha(SHADOW_ALPHA);
        let mut mesh = Mesh::default();
        for p in c {
            mesh.colored_vertex(p, dark);
        }
        for p in outer {
            mesh.colored_vertex(p, Color32::TRANSPARENT);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        for k in 0..4u32 {
            let n = (k + 1) % 4;
            mesh.add_triangle(k, n, 4 + k);
            mesh.add_triangle(n, 4 + n, 4 + k);
        }
        painter.add(mesh);
    }

    fn paint_page(&self, painter: &Painter, i: usize, ppp: f32) {
        let corners = self.page_corners(i);
        let size = self.layout.pages[i];
        let pdf = self.pdf.as_ref();
        match pdf.and_then(|p| p.page_image(i)) {
            Some(img) => {
                let full = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(size.w, size.h));
                self.paint_image(painter, i, &img, full, ppp);
            }
            None => {
                painter.add(Shape::convex_polygon(
                    corners.to_vec(),
                    theme::PAPER,
                    Stroke::NONE,
                ));
            }
        }
        if let Some((img, region)) = pdf.and_then(|p| p.tile_image(i)) {
            self.paint_image(painter, i, &img, region, ppp);
        }
    }

    /// Tint every search match on the visible pages, the current one stronger and outlined.
    /// Pdfium's boxes hug the glyphs, so they're padded to cover ascenders and descenders.
    fn paint_search_hits(&self, painter: &Painter, visible: Range<usize>) {
        let Some(search) = &self.search else {
            return;
        };
        let pad = |r: &Rect| {
            let across = r.width().min(r.height()) * 0.25;
            if r.width() >= r.height() {
                r.expand2(Vec2::new(1.0, across))
            } else {
                r.expand2(Vec2::new(across, 1.0))
            }
        };
        for i in visible {
            let Some(hits) = search.page(i) else {
                continue;
            };
            let offset = self.layout.page_rect(i).min.to_vec2();
            let current = search
                .current
                .filter(|c| c.0 == i)
                .map(|c| hits.hits[c.1].clone());
            for (k, r) in hits.rects.iter().enumerate() {
                let quad = self.doc_quad(pad(r).translate(offset));
                if current.as_ref().is_some_and(|c| c.contains(&k)) {
                    painter.add(Shape::convex_polygon(
                        quad.clone(),
                        theme::SEARCH_CURRENT,
                        Stroke::NONE,
                    ));
                    painter.add(Shape::closed_line(quad, theme::line(1.5, theme::ACCENT)));
                } else {
                    painter.add(Shape::convex_polygon(quad, theme::SEARCH_HIT, Stroke::NONE));
                }
            }
        }
    }

    /// Draw `img` over the page-local `region` (points) of page `i`. Upright, a texture
    /// rendered at the display scale is snapped so texels land 1:1 on physical pixels —
    /// bilinear filtering would otherwise soften every glyph by half a pixel.
    fn paint_image(&self, painter: &Painter, i: usize, img: &PageImage, region: Rect, ppp: f32) {
        let page = self.layout.page_rect(i);
        let doc = region.translate(page.min.to_vec2());
        let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        if self.cam.is_upright() {
            let min = self.cam.doc_to_screen(doc.min, self.viewport);
            let max = self.cam.doc_to_screen(doc.max, self.viewport);
            let tex_scale = img.size[0] as f32 / region.width();
            let display_scale = self.cam.zoom * ppp;
            let rect = if ((tex_scale - display_scale) / display_scale).abs() < 0.01 {
                let snapped = ((min.to_vec2() * ppp).round() / ppp).to_pos2();
                Rect::from_min_size(
                    snapped,
                    Vec2::new(img.size[0] as f32, img.size[1] as f32) / ppp,
                )
            } else {
                Rect::from_min_max(min, max)
            };
            painter.image(img.id, rect, uv, Color32::WHITE);
            return;
        }
        let corners = [
            doc.left_top(),
            doc.right_top(),
            doc.right_bottom(),
            doc.left_bottom(),
        ]
        .map(|p| self.cam.doc_to_screen(p, self.viewport));
        let uvs = [
            uv.left_top(),
            uv.right_top(),
            uv.right_bottom(),
            uv.left_bottom(),
        ];
        let mut mesh = Mesh::with_texture(img.id);
        for k in 0..4 {
            mesh.vertices.push(Vertex {
                pos: corners[k],
                uv: uvs[k],
                color: Color32::WHITE,
            });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(mesh);
    }

    pub(super) fn paint_stroke(
        &self,
        painter: &Painter,
        norm_points: &[[f32; 2]],
        page: usize,
        color: [u8; 4],
        width_pt: f32,
    ) {
        let to_screen = self.norm_to_screen(page);
        let pts: Vec<Pos2> = norm_points.iter().map(|n| to_screen(*n)).collect();
        let col = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
        let width = width_pt * self.cam.zoom;
        paint_polyline(painter, &pts, width, col);
    }

    /// The affine map from page `page`'s normalised coordinates to the screen, computed once
    /// so per-point mapping is two multiply-adds instead of a full camera transform.
    fn norm_to_screen(&self, page: usize) -> impl Fn([f32; 2]) -> Pos2 {
        let r = self.layout.page_rect(page);
        let origin = self.cam.doc_to_screen(r.min, self.viewport);
        let ex = self.cam.doc_to_screen(r.right_top(), self.viewport) - origin;
        let ey = self.cam.doc_to_screen(r.left_bottom(), self.viewport) - origin;
        move |n| origin + ex * n[0] + ey * n[1]
    }

    /// Paint a committed text annotation as a rotated, zoom-scaled galley.
    fn paint_text(&self, painter: &Painter, t: &crate::annotation::TextAnnotation) {
        if t.text.is_empty() {
            return;
        }
        let px = (t.size_pt * self.cam.zoom).max(1.0);
        let col = Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], t.color[3]);
        let galley = painter.layout_no_wrap(t.text.clone(), FontId::proportional(px), col);
        let anchor = self
            .cam
            .doc_to_screen(self.layout.norm_to_doc(t.page, t.pos), self.viewport);
        painter.add(TextShape::new(anchor, galley, col).with_angle(self.cam.rotation));
    }

    /// Screen quad of a doc-space rect (follows the camera rotation).
    fn doc_quad(&self, r: Rect) -> Vec<Pos2> {
        [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ]
        .map(|p| self.cam.doc_to_screen(p, self.viewport))
        .to_vec()
    }

    /// Hover/drag feedback and the cursor for the current tool.
    fn paint_feedback(&self, painter: &Painter, response: &Response) {
        let ctx = self.ctx.clone();
        let hover = ctx.input(|i| i.pointer.hover_pos());
        let over = response.contains_pointer() || !self.gesture.is_none();
        let Some(pos) = hover.filter(|_| over) else {
            return;
        };
        let doc = self.cam.screen_to_doc(pos, self.viewport);
        let on_page = self.layout.page_at_doc(doc).is_some();
        let gesture_active = !self.gesture.is_none();

        // Grab hands for anything that pans.
        let panning = matches!(self.gesture, Gesture::Pan { .. } | Gesture::Rotate { .. });
        if panning {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if self.space_down && !gesture_active {
            ctx.set_cursor_icon(CursorIcon::Grab);
        }
        if let Gesture::Rotate { .. } = self.gesture {
            self.paint_rotation_readout(painter, pos);
        }
        if panning || (self.space_down && !gesture_active) {
            return;
        }

        match self.tool {
            Tool::Nav => ctx.set_cursor_icon(CursorIcon::Grab),
            Tool::Pen | Tool::Highlighter => {
                if !on_page && !gesture_active {
                    ctx.set_cursor_icon(CursorIcon::Grab);
                    return;
                }
                ctx.set_cursor_icon(CursorIcon::None);
                let brush = if self.tool == Tool::Highlighter {
                    self.highlighter
                } else {
                    self.pen
                };
                let r = (brush.size * self.cam.zoom * 0.5).max(1.0);
                paint_brush_cursor(painter, pos, r, brush.color);
            }
            Tool::Eraser => {
                ctx.set_cursor_icon(CursorIcon::None);
                // Preview what a click here would erase.
                if !gesture_active {
                    for id in self.hits_along(pos, pos) {
                        self.paint_erase_preview(painter, id);
                    }
                }
                let r = self.eraser_px;
                painter.circle_filled(pos, r, Color32::from_white_alpha(28));
                painter.circle_stroke(
                    pos,
                    r + 1.0,
                    theme::line(1.0, Color32::from_black_alpha(150)),
                );
                painter.circle_stroke(pos, r, theme::line(1.5, Color32::WHITE));
            }
            Tool::Text => {
                let hovered = match &self.gesture {
                    Gesture::TextDrag { id, .. } => Some(*id),
                    _ if gesture_active => None,
                    _ => self.text_at_doc(doc),
                };
                if let Some(t) = hovered.and_then(|id| self.set.text_by_id(id)) {
                    let quad = self.doc_quad(self.text_bbox_doc(t).expand(2.0 / self.cam.zoom));
                    painter.add(Shape::closed_line(quad, theme::line(1.0, theme::ACCENT)));
                    ctx.set_cursor_icon(if gesture_active {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Move
                    });
                } else if on_page {
                    ctx.set_cursor_icon(CursorIcon::Text);
                } else {
                    ctx.set_cursor_icon(CursorIcon::Grab);
                }
            }
        }
    }

    /// Whiten an annotation the eraser is hovering, so it's clear what will go.
    fn paint_erase_preview(&self, painter: &Painter, id: u64) {
        let veil = Color32::from_white_alpha(170);
        if let Some(s) = self.set.strokes.iter().find(|s| s.id == id) {
            self.paint_stroke(
                painter,
                &s.points,
                s.page,
                veil.to_srgba_unmultiplied(),
                s.width_pt,
            );
        } else if let Some(t) = self.set.text_by_id(id) {
            let quad = self.doc_quad(self.text_bbox_doc(t).expand(1.0 / self.cam.zoom));
            painter.add(Shape::convex_polygon(quad, veil, Stroke::NONE));
        }
    }

    /// Angle pill that follows the pointer during a rotate-drag.
    fn paint_rotation_readout(&self, painter: &Painter, pos: Pos2) {
        // `+ 0.0` turns a rounded -0 into 0, so straight up never reads "-0°".
        let deg = crate::view::wrap_angle(self.cam.rotation)
            .to_degrees()
            .round()
            + 0.0;
        let text = format!("{deg:.0}°");
        let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), theme::TEXT);
        let rect = Rect::from_min_size(pos + Vec2::new(18.0, 18.0), galley.size())
            .expand2(Vec2::new(8.0, 4.0));
        painter.rect(
            rect,
            8.0,
            theme::SURFACE,
            theme::line(1.0, theme::BORDER),
            StrokeKind::Inside,
        );
        painter.galley(rect.center() - galley.size() * 0.5, galley, theme::TEXT);
    }
}

/// The pen/highlighter cursor: the brush footprint in its colour, ringed in dark and light
/// so it reads on any background; a tiny brush gets crosshair ticks so its point is clear.
fn paint_brush_cursor(painter: &Painter, pos: Pos2, r: f32, color: Color32) {
    let [cr, cg, cb, ca] = color.to_srgba_unmultiplied();
    let fill = Color32::from_rgba_unmultiplied(cr, cg, cb, (ca as f32 * 0.55) as u8);
    painter.circle_filled(pos, r, fill);
    painter.circle_stroke(
        pos,
        r + 1.0,
        theme::line(1.0, Color32::from_black_alpha(150)),
    );
    painter.circle_stroke(pos, r, theme::line(1.0, Color32::from_white_alpha(220)));
    if r < 3.0 {
        let tick = theme::line(1.0, Color32::from_black_alpha(170));
        for d in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
            painter.line_segment([pos + d * (r + 3.0), pos + d * (r + 8.0)], tick);
        }
    }
}

/// Paint a polyline `width` px wide with round caps and joins, anti-aliased — the same
/// geometry the exported PDF strokes use. A single point is a dot.
pub fn paint_polyline(painter: &Painter, pts: &[Pos2], width: f32, color: Color32) {
    let mut clean: Vec<Pos2> = Vec::with_capacity(pts.len());
    for &p in pts {
        if clean.last().is_none_or(|q: &Pos2| q.distance(p) > 0.05) {
            clean.push(p);
        }
    }
    let r = width * 0.5;
    match clean.len() {
        0 => {}
        1 => {
            painter.circle_filled(clean[0], r.max(0.5), color);
        }
        _ if width < 1.5 => {
            // Hairlines: egui's own feathered line is the right tool (it fades sub-pixel
            // widths by alpha), and caps are invisible at this size.
            painter.add(Shape::line(clean, Stroke::new(width.max(0.25), color)));
        }
        _ => {
            // Split at sharp corners, where a mitred join would spike; each piece gets
            // round caps, so the corner itself comes out round.
            let mut start = 0;
            for k in 1..clean.len() - 1 {
                let a = (clean[k] - clean[k - 1]).normalized();
                let b = (clean[k + 1] - clean[k]).normalized();
                if a.dot(b) < -0.2 {
                    painter.add(stroke_mesh(
                        &clean[start..=k],
                        r,
                        color,
                        painter.pixels_per_point(),
                    ));
                    start = k;
                }
            }
            painter.add(stroke_mesh(
                &clean[start..],
                r,
                color,
                painter.pixels_per_point(),
            ));
        }
    }
}

/// Tessellate a polyline (≥ 2 distinct points) of half-width `r` into a mesh with
/// semicircular end caps and a 1-physical-pixel feathered edge.
fn stroke_mesh(pts: &[Pos2], r: f32, color: Color32, ppp: f32) -> Mesh {
    let feather = 1.0 / ppp;
    let r_in = (r - feather * 0.5).max(0.0);
    let r_out = r + feather * 0.5;
    let clear = Color32::TRANSPARENT;
    let n = pts.len();
    let perp = |v: Vec2| Vec2::new(-v.y, v.x);

    // Per-vertex offset direction: averaged (mitred) normal, limited so tight curves don't
    // shoot spikes.
    let normals: Vec<Vec2> = (0..n)
        .map(|i| {
            let d_prev = if i > 0 {
                (pts[i] - pts[i - 1]).normalized()
            } else {
                Vec2::ZERO
            };
            let d_next = if i + 1 < n {
                (pts[i + 1] - pts[i]).normalized()
            } else {
                Vec2::ZERO
            };
            let d = (d_prev + d_next).normalized();
            let d = if d == Vec2::ZERO || !d.x.is_finite() {
                d_next + d_prev
            } else {
                d
            };
            let nrm = perp(d);
            let reference = perp(if i + 1 < n { d_next } else { d_prev });
            let cos = nrm.dot(reference).max(0.5);
            nrm / cos
        })
        .collect();

    let mut mesh = Mesh::default();
    // Four vertices per point: left outer (clear), left inner, right inner, right outer.
    for (p, nrm) in pts.iter().zip(&normals) {
        mesh.colored_vertex(*p + *nrm * r_out, clear);
        mesh.colored_vertex(*p + *nrm * r_in, color);
        mesh.colored_vertex(*p - *nrm * r_in, color);
        mesh.colored_vertex(*p - *nrm * r_out, clear);
    }
    for i in 0..n as u32 - 1 {
        let (a, b) = (i * 4, (i + 1) * 4);
        for lane in 0..3 {
            mesh.add_triangle(a + lane, a + lane + 1, b + lane);
            mesh.add_triangle(a + lane + 1, b + lane + 1, b + lane);
        }
    }

    // Round caps: a fan from each end, sharing the strip's end vertices so there's no seam.
    let steps = ((r * ppp).sqrt() * 3.0).clamp(4.0, 24.0) as usize;
    for (end, dir, first) in [
        (pts[0], (pts[0] - pts[1]).normalized(), 0u32),
        (
            pts[n - 1],
            (pts[n - 1] - pts[n - 2]).normalized(),
            (n as u32 - 1) * 4,
        ),
    ] {
        // `first` is the end's +normal outer vertex; the cap sweeps from +normal, through
        // `dir` (pointing out of the stroke), to −normal.
        let nrm = normals[(first / 4) as usize].normalized();
        let side = if perp(nrm).dot(dir) > 0.0 { 1.0 } else { -1.0 };
        let centre = mesh.vertices.len() as u32;
        mesh.colored_vertex(end, color);
        let mut prev_in = first + 1;
        let mut prev_out = first;
        for k in 1..steps {
            let a = std::f32::consts::PI * k as f32 / steps as f32 * side;
            let (s, c) = a.sin_cos();
            let v = Vec2::new(nrm.x * c - nrm.y * s, nrm.x * s + nrm.y * c);
            let vi = mesh.vertices.len() as u32;
            mesh.colored_vertex(end + v * r_in, color);
            mesh.colored_vertex(end + v * r_out, clear);
            mesh.add_triangle(centre, prev_in, vi);
            mesh.add_triangle(prev_in, prev_out, vi);
            mesh.add_triangle(prev_out, vi + 1, vi);
            prev_in = vi;
            prev_out = vi + 1;
        }
        let (last_in, last_out) = (first + 2, first + 3);
        mesh.add_triangle(centre, prev_in, last_in);
        mesh.add_triangle(prev_in, prev_out, last_in);
        mesh.add_triangle(prev_out, last_out, last_in);
    }
    mesh
}
