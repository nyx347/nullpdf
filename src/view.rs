//! Camera (pan/zoom/free-rotation) and continuous-mode page layout.
//!
//! Everything is expressed in three spaces:
//! * **page-normalised**: `0..1` within a single page (how strokes are stored)
//! * **doc**: PDF points, all pages stacked vertically and centred on `x = 0`
//! * **screen**: egui pixels
//!
//! The camera is rotation-invariant: it stores the doc point shown at the viewport
//! centre, a zoom (screen px per doc point), and a free rotation angle.

use egui::{Pos2, Rect, Vec2};

/// Gap between consecutive pages, in doc points.
const PAGE_GAP: f32 = 16.0;

/// Size of a single page in doc points.
#[derive(Clone, Copy, Debug)]
pub struct PageSize {
    pub w: f32,
    pub h: f32,
}

/// Continuous vertical layout of all pages. Page `i` occupies `page_rect(i)` in doc space,
/// horizontally centred on `x = 0`, stacked top-to-bottom starting at `y = 0`.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub pages: Vec<PageSize>,
    /// `y_offsets[i]` is the doc-space top edge of page `i`.
    y_offsets: Vec<f32>,
    total_height: f32,
    max_width: f32,
}

impl Layout {
    pub fn new(pages: Vec<PageSize>) -> Self {
        let mut y_offsets = Vec::with_capacity(pages.len());
        let mut y = 0.0;
        let mut max_width = 0.0f32;
        for p in &pages {
            y_offsets.push(y);
            y += p.h + PAGE_GAP;
            max_width = max_width.max(p.w);
        }
        let total_height = (y - PAGE_GAP).max(0.0);
        Self {
            pages,
            y_offsets,
            total_height,
            max_width,
        }
    }

    pub fn total_height(&self) -> f32 {
        self.total_height
    }

    pub fn max_width(&self) -> f32 {
        self.max_width
    }

    /// Doc-space rectangle of page `i` (centred on `x = 0`).
    pub fn page_rect(&self, i: usize) -> Rect {
        let p = self.pages[i];
        let top = self.y_offsets[i];
        Rect::from_min_max(
            Pos2::new(-p.w * 0.5, top),
            Pos2::new(p.w * 0.5, top + p.h),
        )
    }

    /// Find the page containing doc point `p`, returning `(index, normalised xy)`.
    /// Points in the gap between pages snap to the nearest page so dragging across a
    /// boundary still draws on the page where the stroke started (caller locks the page).
    pub fn page_at_doc(&self, p: Pos2) -> Option<(usize, [f32; 2])> {
        for i in 0..self.pages.len() {
            let r = self.page_rect(i);
            if p.y >= r.min.y && p.y <= r.max.y {
                let nx = ((p.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                let ny = ((p.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                return Some((i, [nx, ny]));
            }
        }
        None
    }

    /// Convert a normalised page point on page `i` to doc space.
    pub fn norm_to_doc(&self, i: usize, n: [f32; 2]) -> Pos2 {
        let r = self.page_rect(i);
        Pos2::new(r.min.x + n[0] * r.width(), r.min.y + n[1] * r.height())
    }
}

/// Rotation-invariant camera over the document.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Doc-space point displayed at the viewport centre.
    pub center: Pos2,
    /// Screen pixels per doc point.
    pub zoom: f32,
    /// Free rotation in radians (clockwise on screen).
    pub rotation: f32,
    /// True until the first frame has sized/positioned the view to fit page width.
    pub initialized: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Pos2::ZERO,
            zoom: 1.0,
            rotation: 0.0,
            initialized: false,
        }
    }
}

impl Camera {
    /// One-time fit: zoom so the widest page fills ~90% of the viewport width, and place
    /// the document's top edge near the top of the viewport.
    pub fn fit(&mut self, layout: &Layout, viewport: Rect) {
        let max_w = layout.max_width().max(1.0);
        self.zoom = (viewport.width() * 0.9 / max_w).clamp(0.05, 8.0);
        self.center = Pos2::new(0.0, viewport.height() * 0.5 / self.zoom);
        self.rotation = 0.0;
        self.initialized = true;
    }

    fn rot(v: Vec2, angle: f32) -> Vec2 {
        let (s, c) = angle.sin_cos();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// doc point -> screen pixel.
    pub fn doc_to_screen(&self, p: Pos2, viewport: Rect) -> Pos2 {
        let v = (p - self.center) * self.zoom;
        viewport.center() + Self::rot(v, self.rotation)
    }

    /// screen pixel -> doc point.
    pub fn screen_to_doc(&self, s: Pos2, viewport: Rect) -> Pos2 {
        let v = Self::rot(s - viewport.center(), -self.rotation) / self.zoom;
        self.center + v
    }

    /// Pan by a screen-space drag delta, keeping content under the cursor.
    pub fn pan(&mut self, screen_delta: Vec2) {
        self.center -= Self::rot(screen_delta, -self.rotation) / self.zoom;
    }

    /// Scroll the document vertically (advances through the page stack regardless of
    /// rotation), measured in screen pixels.
    pub fn scroll(&mut self, screen_pixels: f32) {
        self.center.y += screen_pixels / self.zoom;
    }

    /// Zoom by `factor` around a fixed screen anchor (keeps the doc point under the
    /// cursor stationary).
    pub fn zoom_around(&mut self, factor: f32, anchor: Pos2, viewport: Rect) {
        let before = self.screen_to_doc(anchor, viewport);
        self.zoom = (self.zoom * factor).clamp(0.05, 12.0);
        let after = self.screen_to_doc(anchor, viewport);
        self.center += before - after;
    }

    /// Keep the document centre within reach so it can't be scrolled/panned into the
    /// void. Allows a margin of one viewport-half of overscroll past each edge.
    pub fn clamp_to(&mut self, layout: &Layout, viewport: Rect) {
        let margin_y = (viewport.height() * 0.5 / self.zoom).max(0.0);
        let margin_x = (viewport.width() * 0.5 / self.zoom).max(0.0);
        let half_w = layout.max_width() * 0.5;
        self.center.y = self.center.y.clamp(-margin_y, layout.total_height() + margin_y);
        self.center.x = self.center.x.clamp(-half_w - margin_x, half_w + margin_x);
    }

    /// Rotate around the viewport centre by `delta` radians, snapping to the nearest
    /// 90° axis when within ~5°.
    pub fn rotate_by(&mut self, delta: f32) {
        self.rotation += delta;
        const SNAP: f32 = 5.0 * std::f32::consts::PI / 180.0;
        let quarter = std::f32::consts::FRAC_PI_2;
        let nearest = (self.rotation / quarter).round() * quarter;
        if (self.rotation - nearest).abs() < SNAP {
            self.rotation = nearest;
        }
        // Keep the angle bounded.
        let tau = std::f32::consts::TAU;
        self.rotation = self.rotation.rem_euclid(tau);
    }
}
