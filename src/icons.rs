//! Hand-painted vector icons. Each icon is drawn on a 24×24 unit grid, scaled into the
//! target rect, with a constant-width round-jointed stroke so they stay crisp at any size
//! and DPI. (The bundled icon font lacks a highlighter, eraser and folder, and mixing
//! glyph styles looks worse than drawing a consistent set.)

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Hand,
    Pen,
    Highlighter,
    Eraser,
    Text,
    Undo,
    Redo,
    Open,
    Export,
    Help,
    Rotate,
    Search,
    Bookmark,
    /// Chevrons pointing up / down (previous / next match).
    Up,
    Down,
    Close,
}

/// Draws polylines in 24-unit icon space.
struct Canvas<'a> {
    painter: &'a Painter,
    origin: Pos2,
    unit: f32,
    stroke: Stroke,
    mirror: bool,
}

impl Canvas<'_> {
    fn p(&self, (x, y): (f32, f32)) -> Pos2 {
        let x = if self.mirror { 24.0 - x } else { x };
        self.origin + Vec2::new(x, y) * self.unit
    }

    /// Open polyline with round caps and joins.
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&q| self.p(q)).collect();
        let r = self.stroke.width * 0.5;
        for &q in &pts {
            self.painter.circle_filled(q, r, self.stroke.color);
        }
        self.painter.add(Shape::line(pts, self.stroke));
    }

    /// Closed polygon outline with round joins.
    fn closed(&self, pts: &[(f32, f32)]) {
        let mut v = pts.to_vec();
        v.push(pts[0]);
        self.line(&v);
    }

    fn circle(&self, c: (f32, f32), r: f32) {
        self.painter
            .circle_stroke(self.p(c), r * self.unit, self.stroke);
    }
}

/// Points along a circular arc (y-down angles, so increasing angle runs clockwise).
fn arc(c: (f32, f32), r: f32, a0: f32, a1: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// Points of a quadratic Bézier from `a` to `b` with control `c`.
fn quad(a: (f32, f32), c: (f32, f32), b: (f32, f32), steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            (
                u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
                u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
            )
        })
        .collect()
}

/// A slanted tool body (pen/marker/eraser) along the 45° up-right axis.
struct Slant {
    origin: (f32, f32),
}

impl Slant {
    const U: (f32, f32) = (
        std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    );
    const V: (f32, f32) = (
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    );

    /// Point `along` the axis and `across` it (positive = lower-right side).
    fn at(&self, along: f32, across: f32) -> (f32, f32) {
        (
            self.origin.0 + Self::U.0 * along + Self::V.0 * across,
            self.origin.1 + Self::U.1 * along + Self::V.1 * across,
        )
    }

    /// Semicircular end cap centred `along` the axis, from the +across side to the −across
    /// side, bulging forward.
    fn cap(&self, along: f32, r: f32) -> Vec<(f32, f32)> {
        let c = self.at(along, 0.0);
        let base = Self::V.1.atan2(Self::V.0); // angle of +across
        arc(c, r, base, base - PI, 10)
    }
}

/// Paint `icon` centred in `rect` (square-ish), in `color`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    let unit = side / 24.0;
    let origin = rect.center() - Vec2::splat(side * 0.5);
    let stroke = Stroke::new((1.7 * unit).max(1.25), color);
    let mut c = Canvas {
        painter,
        origin,
        unit,
        stroke,
        mirror: false,
    };

    match icon {
        Icon::Hand => {
            // Separate finger strokes: an outlined hand blurs into a mitten at toolbar
            // size. The pinky runs into the palm edge, which wraps round into the thumb.
            c.line(&[(7.0, 8.0), (7.0, 15.0)]);
            c.line(&[(10.5, 4.6), (10.5, 12.8)]);
            c.line(&[(14.0, 5.4), (14.0, 12.8)]);
            let mut palm = vec![(17.5, 8.4), (17.5, 14.0)];
            palm.extend(quad((17.5, 14.0), (17.5, 21.2), (12.2, 21.2), 10));
            palm.extend(quad((12.2, 21.2), (8.6, 21.2), (6.6, 18.6), 8));
            palm.push((3.9, 14.9));
            palm.extend(arc((5.05, 13.95), 1.5, PI * 0.74, PI * 1.9, 6));
            palm.push((7.0, 15.0));
            c.line(&palm);
        }
        Icon::Pen => {
            let s = Slant {
                origin: (4.0, 20.0),
            };
            let w = 2.6;
            let mut path = vec![s.at(0.0, 0.0), s.at(4.2, w), s.at(17.0, w)];
            path.extend(s.cap(17.0, w));
            path.extend([s.at(4.2, -w), s.at(0.0, 0.0)]);
            c.line(&path);
            c.line(&[s.at(4.2, w), s.at(4.2, -w)]);
            c.line(&[s.at(13.6, w), s.at(13.6, -w)]);
        }
        Icon::Highlighter => {
            let s = Slant {
                origin: (5.6, 17.6),
            };
            let w = 3.3;
            let t = 1.5;
            let mut path = vec![s.at(0.0, t), s.at(4.6, w), s.at(15.0, w)];
            path.extend(s.cap(15.0, w));
            path.extend([s.at(4.6, -w), s.at(0.0, -t)]);
            c.closed(&path);
            c.line(&[s.at(4.6, w), s.at(4.6, -w)]);
            c.line(&[(3.2, 21.6), (13.2, 21.6)]);
        }
        Icon::Eraser => {
            let s = Slant {
                origin: (6.9, 17.1),
            };
            let w = 4.4;
            c.closed(&[s.at(0.0, w), s.at(15.6, w), s.at(15.6, -w), s.at(0.0, -w)]);
            c.line(&[s.at(6.4, w), s.at(6.4, -w)]);
            c.line(&[(11.6, 21.0), (20.4, 21.0)]);
        }
        Icon::Text => {
            c.line(&[(5.5, 8.2), (5.5, 5.5), (18.5, 5.5), (18.5, 8.2)]);
            c.line(&[(12.0, 5.5), (12.0, 18.5)]);
            c.line(&[(9.2, 18.5), (14.8, 18.5)]);
        }
        Icon::Undo | Icon::Redo => {
            c.mirror = icon == Icon::Redo;
            c.line(&[(8.6, 4.6), (4.2, 9.0), (8.6, 13.4)]);
            let mut path = vec![(4.2, 9.0), (14.0, 9.0)];
            path.extend(arc((14.0, 14.25), 5.25, -FRAC_PI_2, FRAC_PI_2, 12));
            path.push((9.6, 19.5));
            c.line(&path);
        }
        Icon::Open => {
            c.closed(&[
                (3.0, 18.8),
                (3.0, 6.0),
                (9.2, 6.0),
                (11.2, 8.2),
                (21.0, 8.2),
                (21.0, 18.8),
            ]);
            c.line(&[(3.0, 11.0), (21.0, 11.0)]);
        }
        Icon::Export => {
            c.line(&[(12.0, 15.0), (12.0, 3.8)]);
            c.line(&[(7.6, 8.2), (12.0, 3.8), (16.4, 8.2)]);
            c.line(&[(4.6, 12.6), (4.6, 20.0), (19.4, 20.0), (19.4, 12.6)]);
        }
        Icon::Help => {
            c.circle((12.0, 12.0), 9.0);
            let mut q = arc((12.0, 9.4), 2.7, PI * 1.05, PI * 2.3, 10);
            q.push((12.0, 13.6));
            c.line(&q);
            painter.circle_filled(c.p((12.0, 16.9)), stroke.width * 0.75, color);
        }
        Icon::Rotate => {
            let a0 = -FRAC_PI_2 + 0.5;
            let a1 = a0 + 4.9;
            c.line(&arc((12.0, 12.0), 7.0, a0, a1, 18));
            // Arrowhead at the end of the arc, pointing along its (clockwise) tangent.
            let end = (12.0 + 7.0 * a1.cos(), 12.0 + 7.0 * a1.sin());
            let tangent = (-a1.sin(), a1.cos());
            let normal = (a1.cos(), a1.sin());
            let back = |d: f32, n: f32| {
                (
                    end.0 - tangent.0 * d + normal.0 * n,
                    end.1 - tangent.1 * d + normal.1 * n,
                )
            };
            c.line(&[back(3.2, 2.6), end, back(3.2, -2.6)]);
        }
        Icon::Search => {
            c.circle((10.4, 10.4), 6.4);
            c.line(&[(15.1, 15.1), (20.2, 20.2)]);
        }
        Icon::Bookmark => {
            c.closed(&[
                (6.5, 3.8),
                (17.5, 3.8),
                (17.5, 20.4),
                (12.0, 15.6),
                (6.5, 20.4),
            ]);
        }
        Icon::Up => c.line(&[(6.4, 14.8), (12.0, 9.2), (17.6, 14.8)]),
        Icon::Down => c.line(&[(6.4, 9.2), (12.0, 14.8), (17.6, 9.2)]),
        Icon::Close => {
            c.line(&[(7.0, 7.0), (17.0, 17.0)]);
            c.line(&[(17.0, 7.0), (7.0, 17.0)]);
        }
    }
}

/// A hue wheel ring (for the custom-colour button), built as a coloured triangle mesh.
pub fn paint_hue_ring(painter: &Painter, center: Pos2, r_outer: f32, r_inner: f32) {
    let n = 48;
    let mut mesh = egui::Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = t * TAU - FRAC_PI_2;
        let dir = Vec2::new(a.cos(), a.sin());
        let col: Color32 = egui::ecolor::Hsva::new(t, 0.85, 1.0, 1.0).into();
        mesh.colored_vertex(center + dir * r_outer, col);
        mesh.colored_vertex(center + dir * r_inner, col);
    }
    for i in 0..n as u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    painter.add(mesh);
}

/// The window icon: a white page with lines of text and a red pen swoosh on a dark tile,
/// rasterised here (4×4 supersampled) so the binary needs no image assets. The AppImage's
/// and the Windows exe's icons are renders of the same design by `packaging/render-icon.py`:
/// keep the two in step.
pub fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    const SS: usize = 4;
    let rounded = |p: Vec2, min: Vec2, max: Vec2, r: f32| {
        let c = (min + max) * 0.5;
        let half = (max - min) * 0.5 - Vec2::splat(r);
        let d = (p - c).abs() - half;
        Vec2::new(d.x.max(0.0), d.y.max(0.0)).length() + d.x.max(d.y).min(0.0) - r <= 0.0
    };
    let swoosh: Vec<Pos2> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let (a, c, b) = (
                Pos2::new(13.0, 47.0),
                Pos2::new(30.0, 60.0),
                Pos2::new(51.0, 31.0),
            );
            let u = 1.0 - t;
            Pos2::new(
                u * u * a.x + 2.0 * u * t * c.x + t * t * b.x,
                u * u * a.y + 2.0 * u * t * c.y + t * t * b.y,
            )
        })
        .collect();
    let on_swoosh = |p: Pos2| {
        swoosh.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0);
            p.distance(a + ab * t) <= 2.6
        })
    };
    // Back to front: tile, page, text lines, ink.
    let shade = |p: Pos2| -> [f32; 4] {
        let v = p.to_vec2();
        if on_swoosh(p) {
            return [220.0, 40.0, 40.0, 255.0];
        }
        if rounded(v, Vec2::new(18.0, 10.0), Vec2::new(46.0, 54.0), 3.0) {
            let line = (0..5).any(|k| {
                let y = 18.0 + k as f32 * 6.0;
                let end = if k == 4 { 33.0 } else { 40.0 };
                v.y >= y && v.y < y + 2.2 && v.x >= 23.0 && v.x < end
            });
            return if line {
                [196.0, 198.0, 206.0, 255.0]
            } else {
                [250.0, 250.0, 248.0, 255.0]
            };
        }
        if rounded(v, Vec2::splat(3.0), Vec2::splat(61.0), 14.0) {
            return [38.0, 39.0, 44.0, 255.0];
        }
        [0.0; 4]
    };
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Average premultiplied samples, then un-premultiply.
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = Pos2::new(
                        x as f32 + (sx as f32 + 0.5) / SS as f32,
                        y as f32 + (sy as f32 + 0.5) / SS as f32,
                    );
                    let c = shade(p);
                    let a = c[3] / 255.0;
                    for k in 0..3 {
                        acc[k] += c[k] * a;
                    }
                    acc[3] += c[3];
                }
            }
            let n = (SS * SS) as f32;
            let alpha = acc[3] / n;
            let unpremul = |v: f32| {
                if alpha > 0.0 {
                    (v / n / (alpha / 255.0)).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            rgba.extend([
                unpremul(acc[0]),
                unpremul(acc[1]),
                unpremul(acc[2]),
                alpha.round() as u8,
            ]);
        }
    }
    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}
