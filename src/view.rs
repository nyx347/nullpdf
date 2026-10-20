//! Camera (pan/zoom/free-rotation) and continuous-mode page layout.
//!
//! Everything is expressed in three spaces:
//! * **page-normalised**: `0..1` within a single page (how strokes are stored)
//! * **doc**: PDF points, all pages stacked vertically and centred on `x = 0`
//! * **screen**: egui pixels
//!
//! The camera is rotation-invariant: it stores the doc point shown at the viewport
//! centre, a zoom (screen px per doc point), and a free rotation angle.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ops::Range;

use egui::{Pos2, Rect, Vec2};

/// Gap between consecutive pages, in doc points.
const PAGE_GAP: f32 = 16.0;

/// Zoom limits (screen px per doc point).
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 16.0;

/// How far (screen px) the document may be dragged past its edges.
const OVERSCROLL_PX: f32 = 28.0;

/// Exponential approach rate (1/s) of animated camera moves: ~95% of the way in 0.18 s.
const GLIDE_RATE: f32 = 17.0;

/// Rotation snaps to the nearest 90° axis when within this angle.
const AXIS_SNAP: f32 = 5.0 * PI / 180.0;
/// Rotation step used while Shift is held during a rotate-drag.
const FINE_STEP: f32 = 15.0 * PI / 180.0;

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

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Doc-space bounding box of the whole document.
    pub fn bounds(&self) -> Rect {
        Rect::from_min_max(
            Pos2::new(-self.max_width * 0.5, 0.0),
            Pos2::new(self.max_width * 0.5, self.total_height),
        )
    }

    /// Doc-space rectangle of page `i` (centred on `x = 0`).
    pub fn page_rect(&self, i: usize) -> Rect {
        let p = self.pages[i];
        let top = self.y_offsets[i];
        Rect::from_min_max(Pos2::new(-p.w * 0.5, top), Pos2::new(p.w * 0.5, top + p.h))
    }

    /// The page containing doc point `p`, with `p` in that page's normalised coordinates.
    /// Points in the gaps between pages, or beside them, hit nothing.
    pub fn page_at_doc(&self, p: Pos2) -> Option<(usize, [f32; 2])> {
        let i = self.page_near_y(p.y)?;
        let r = self.page_rect(i);
        r.contains(p).then(|| (i, self.doc_to_norm(i, p)))
    }

    /// The page whose vertical span contains doc `y`, or the nearest one to it.
    pub fn page_near_y(&self, y: f32) -> Option<usize> {
        if self.pages.is_empty() {
            return None;
        }
        // First page whose bottom edge is at or below `y`'s position.
        let i = self
            .y_offsets
            .partition_point(|&top| top <= y)
            .saturating_sub(1);
        // In a gap: choose whichever neighbour is closer.
        let bottom = self.y_offsets[i] + self.pages[i].h;
        if y > bottom && i + 1 < self.pages.len() && self.y_offsets[i + 1] - y < y - bottom {
            return Some(i + 1);
        }
        Some(i)
    }

    /// Indices of pages whose rectangles intersect doc-space `r`.
    pub fn pages_in(&self, r: Rect) -> Range<usize> {
        let start = self
            .y_offsets
            .iter()
            .zip(&self.pages)
            .position(|(top, p)| top + p.h >= r.min.y)
            .unwrap_or(self.pages.len());
        let end = self.y_offsets.partition_point(|&top| top <= r.max.y);
        start..end.max(start)
    }

    /// Convert a normalised page point on page `i` to doc space.
    pub fn norm_to_doc(&self, i: usize, n: [f32; 2]) -> Pos2 {
        let r = self.page_rect(i);
        Pos2::new(r.min.x + n[0] * r.width(), r.min.y + n[1] * r.height())
    }

    /// Convert a doc point to page `i`'s normalised coordinates (not clamped).
    pub fn doc_to_norm(&self, i: usize, p: Pos2) -> [f32; 2] {
        let r = self.page_rect(i);
        [(p.x - r.min.x) / r.width(), (p.y - r.min.y) / r.height()]
    }
}

/// Rotation-invariant camera over the document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Doc-space point displayed at the viewport centre.
    pub center: Pos2,
    /// Screen pixels per doc point.
    pub zoom: f32,
    /// Free rotation in radians (clockwise on screen), kept in `0..TAU`.
    pub rotation: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Pos2::ZERO,
            zoom: 1.0,
            rotation: 0.0,
        }
    }
}

impl Camera {
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

    /// True when the view is upright (rotation is an exact multiple of 360°).
    pub fn is_upright(&self) -> bool {
        self.rotation == 0.0
    }

    /// Pan by a screen-space drag delta, keeping content under the cursor.
    pub fn pan(&mut self, screen_delta: Vec2) {
        self.center -= Self::rot(screen_delta, -self.rotation) / self.zoom;
    }

    /// Zoom by `factor` around a fixed screen anchor (keeps the doc point under the
    /// cursor stationary).
    pub fn zoom_around(&mut self, factor: f32, anchor: Pos2, viewport: Rect) {
        self.set_zoom_around(self.zoom * factor, anchor, viewport);
    }

    /// Set an absolute zoom, keeping the doc point under `anchor` stationary.
    pub fn set_zoom_around(&mut self, zoom: f32, anchor: Pos2, viewport: Rect) {
        let before = self.screen_to_doc(anchor, viewport);
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.screen_to_doc(anchor, viewport);
        self.center += before - after;
    }

    /// Half-extents, in doc units along the doc axes, of the axis-aligned box that covers a
    /// screen rectangle of `size` at the current zoom and rotation.
    pub fn doc_half_extent(&self, size: Vec2) -> Vec2 {
        let (s, c) = self.rotation.sin_cos();
        let (s, c) = (s.abs(), c.abs());
        Vec2::new(c * size.x + s * size.y, s * size.x + c * size.y) * (0.5 / self.zoom)
    }

    /// Screen-space bounding-box size of a doc-space box of `size` (at this zoom/rotation).
    pub fn screen_extent(&self, size: Vec2) -> Vec2 {
        let (s, c) = self.rotation.sin_cos();
        let (s, c) = (s.abs(), c.abs());
        Vec2::new(c * size.x + s * size.y, s * size.x + c * size.y) * self.zoom
    }

    /// The camera centre that would put doc point `doc` at screen point `screen`,
    /// keeping the current zoom and rotation.
    pub fn center_for(&self, doc: Pos2, screen: Pos2, viewport: Rect) -> Pos2 {
        doc - Self::rot(screen - viewport.center(), -self.rotation) / self.zoom
    }

    /// Doc-space bounding box of everything visible in screen rect `r`.
    pub fn doc_bbox(&self, r: Rect, viewport: Rect) -> Rect {
        let pts = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ]
        .map(|p| self.screen_to_doc(p, viewport));
        Rect::from_points(&pts)
    }

    /// Keep the document within reach of `safe` (the part of the viewport not covered by
    /// floating chrome). Along each doc axis, a document smaller than the view is centred;
    /// a larger one can be scrolled until its edge meets the view edge, plus a small
    /// overscroll.
    pub fn clamp_to(&mut self, layout: &Layout, viewport: Rect, safe: Rect) {
        if layout.is_empty() {
            return;
        }
        let half = self.doc_half_extent(safe.size());
        let pad = OVERSCROLL_PX / self.zoom;
        let bounds = layout.bounds();
        // Clamp the doc point under the safe-area centre, then map back to the camera centre.
        let offset = Self::rot(safe.center() - viewport.center(), -self.rotation) / self.zoom;
        let mut s = self.center + offset;
        let clamp_axis = |v: f32, lo: f32, hi: f32, half: f32| {
            if hi - lo + 2.0 * pad <= 2.0 * half {
                (lo + hi) * 0.5
            } else {
                v.clamp(lo + half - pad, hi - half + pad)
            }
        };
        s.x = clamp_axis(s.x, bounds.min.x, bounds.max.x, half.x);
        s.y = clamp_axis(s.y, bounds.min.y, bounds.max.y, half.y);
        self.center = s - offset;
    }

    /// Move a fraction of the way toward `target` for a frame of `dt` seconds: centre in doc
    /// space, zoom in log space, rotation along the shortest arc. Returns `false` (having
    /// snapped exactly onto the target) once the remaining distance is imperceptible.
    pub fn step_toward(&mut self, target: &Camera, dt: f32) -> bool {
        let a = 1.0 - (-dt.max(0.0) * GLIDE_RATE).exp();
        let dz = target.zoom.ln() - self.zoom.ln();
        let dr = wrap_angle(target.rotation - self.rotation);
        let dc = target.center - self.center;
        let settled = dc.length() * self.zoom < 0.25 && dz.abs() < 5e-4 && dr.abs() < 5e-4;
        if settled {
            *self = *target;
            return false;
        }
        self.zoom = (self.zoom.ln() + dz * a).exp();
        self.rotation = (self.rotation + dr * a).rem_euclid(TAU);
        self.center += dc * a;
        true
    }
}

/// Wrap an angle into `(-PI, PI]`.
pub fn wrap_angle(a: f32) -> f32 {
    let a = a.rem_euclid(TAU);
    if a > PI {
        a - TAU
    } else {
        a
    }
}

/// Map a raw (unsnapped) rotation to the displayed one: Shift-style `fine` mode quantises
/// to 15° steps; otherwise angles within 5° of a 90° axis snap onto it. Callers keep the raw
/// angle across frames so a slow drag can still leave an axis.
pub fn snap_rotation(raw: f32, fine: bool) -> f32 {
    let snapped = if fine {
        (raw / FINE_STEP).round() * FINE_STEP
    } else {
        let nearest = (raw / FRAC_PI_2).round() * FRAC_PI_2;
        if (raw - nearest).abs() < AXIS_SNAP {
            nearest
        } else {
            raw
        }
    };
    let r = snapped.rem_euclid(TAU);
    // Exact multiples of 360° come back as 0 or TAU depending on rounding; normalise to 0.
    if (r - TAU).abs() < 1e-5 {
        0.0
    } else {
        r
    }
}

/// The next 90° axis from `rotation` in direction `dir` (+1 clockwise, -1 counter-clockwise).
/// An angle already on an axis moves a full quarter turn.
pub fn next_axis(rotation: f32, dir: f32) -> f32 {
    let q = rotation / FRAC_PI_2;
    let steps = if dir > 0.0 {
        (q + 1e-3).floor() + 1.0
    } else {
        (q - 1e-3).ceil() - 1.0
    };
    let r = (steps * FRAC_PI_2).rem_euclid(TAU);
    if (r - TAU).abs() < 1e-5 || r.abs() < 1e-5 {
        0.0
    } else {
        r
    }
}
