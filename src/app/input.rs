//! Keyboard and pointer input.
//!
//! Keys go through the keymap to [`Action`]s. Pointer gestures are driven from raw
//! press/move/release events rather than egui's drag detection, which only reports a drag
//! after the pointer has travelled several pixels — that would clip the start of every
//! stroke and make taps (dots, single erases) impossible.

use std::f32::consts::{FRAC_PI_2, PI};

use egui::{Event, Key, PointerButton, Pos2, Rect, Response, Vec2};

use super::App;
use crate::annotation::{chaikin, simplify, Annotation, Stroke, StrokeKind};
use crate::keymap::{self, Action};
use crate::tools::{ActiveStroke, Gesture, Tool};
use crate::view::{snap_rotation, wrap_angle};

/// Movement (screen px) before a Text-tool press becomes a drag rather than a click.
const DRAG_SLOP: f32 = 4.0;
/// Freehand samples closer than this (screen px) to the previous kept one are dropped.
const MIN_SAMPLE_PX: f32 = 0.75;
/// Max deviation (screen px at drawing zoom) when simplifying a committed freehand stroke.
const SIMPLIFY_PX: f32 = 0.2;
/// Straight lines within this angle of the page's horizontal/vertical snap onto it.
const LINE_SNAP: f32 = 5.0 * PI / 180.0;
/// Multiplier applied to wheel scrolling for a slightly brisker feel.
const SCROLL_SPEED: f32 = 1.4;
/// A trackpad/touch twist counts as one gesture until it pauses this long (s).
const TWIST_IDLE: f64 = 0.3;

impl App {
    // --- keyboard ------------------------------------------------------------------

    pub(super) fn handle_keyboard(&mut self) {
        let ctx = self.ctx.clone();
        // While a text editor has focus, bare keys are typing, not shortcuts.
        let typing = ctx.text_edit_focused();
        let mut actions = std::mem::take(&mut self.queued);
        let mut space_tap = None;
        ctx.input(|i| {
            self.mods = i.modifiers;
            for e in &i.events {
                let Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = e
                else {
                    if matches!(e, Event::WindowFocused(false)) {
                        self.space_down = false;
                    }
                    continue;
                };
                // Space is spring-loaded: held, it turns any left-drag into a pan; tapped,
                // it scrolls a screen (on release, once we know it wasn't a hold).
                if *key == Key::Space && !typing && !modifiers.command {
                    if *pressed && !*repeat {
                        self.space_down = true;
                        self.space_used = false;
                    } else if !*pressed {
                        if self.space_down && !self.space_used {
                            space_tap = Some(modifiers.shift);
                        }
                        self.space_down = false;
                    }
                    continue;
                }
                if !*pressed {
                    continue;
                }
                if let Some(a) = keymap::lookup(*key, *modifiers, typing) {
                    if !*repeat || a.repeats() {
                        actions.push(a);
                    }
                }
            }
        });
        if let Some(shift) = space_tap {
            actions.push(if shift {
                Action::ScreenUp
            } else {
                Action::ScreenDown
            });
        }
        for a in actions {
            self.dispatch(a);
        }
    }

    // --- pointer -------------------------------------------------------------------

    /// Route this frame's pointer events into gestures, then wheel/pinch input.
    pub(super) fn handle_pointer(&mut self, response: &Response) {
        let ctx = self.ctx.clone();
        let (events, any_down) = ctx.input(|i| (i.events.clone(), i.pointer.any_down()));
        let over_canvas = response.contains_pointer() || response.is_pointer_button_down_on();
        for e in &events {
            match e {
                Event::PointerButton {
                    button: PointerButton::Extra1,
                    pressed: true,
                    ..
                } => self.dispatch(Action::PrevPage),
                Event::PointerButton {
                    button: PointerButton::Extra2,
                    pressed: true,
                    ..
                } => self.dispatch(Action::NextPage),
                Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers,
                } => {
                    self.mods = *modifiers;
                    if self.gesture.is_none() && over_canvas && self.viewport.contains(*pos) {
                        self.begin_gesture(*button, *pos);
                    }
                }
                Event::PointerMoved(pos) => self.continue_gesture(*pos),
                Event::PointerButton {
                    pos,
                    button,
                    pressed: false,
                    modifiers,
                } if self.gesture_button == Some(*button) => {
                    // Use the modifiers as they were at the release itself: letting go of
                    // the button and Shift together often lands in a single frame.
                    self.mods = *modifiers;
                    self.continue_gesture(*pos);
                    self.finish_gesture(true);
                }
                Event::WindowFocused(false) => self.finish_gesture(true),
                _ => {}
            }
        }
        // Safety net for a release we never saw (e.g. it happened outside the window).
        if !self.gesture.is_none() && !any_down {
            self.finish_gesture(true);
        }
        if over_canvas || !self.gesture.is_none() {
            self.handle_wheel();
        }
    }

    fn begin_gesture(&mut self, button: PointerButton, pos: Pos2) {
        // A press outside an open text editor commits it; a left press does nothing else.
        if self.text_edit.is_some() {
            self.commit_text();
            if button == PointerButton::Primary {
                self.gesture = Gesture::Swallowed;
                self.gesture_button = Some(button);
                return;
            }
        }
        if self.layout.is_empty() {
            return;
        }
        // Direct manipulation always wins over an animated move.
        self.glide = None;
        let doc = self.cam.screen_to_doc(pos, self.viewport);
        let gesture = match button {
            PointerButton::Middle => Gesture::Pan { last: pos },
            PointerButton::Secondary => Gesture::Rotate {
                last_angle: (pos - self.viewport.center()).angle(),
                raw: self.cam.rotation,
                start: self.cam.rotation,
            },
            PointerButton::Primary if self.space_down => {
                self.space_used = true;
                Gesture::Pan { last: pos }
            }
            PointerButton::Primary => match self.tool {
                Tool::Nav => Gesture::Pan { last: pos },
                Tool::Pen | Tool::Highlighter => match self.layout.page_at_doc(doc) {
                    Some((page, n)) => Gesture::Draw(ActiveStroke {
                        page,
                        points: vec![n],
                        last_screen: pos,
                    }),
                    // Nothing to draw on off the page: dragging there pans instead.
                    None => Gesture::Pan { last: pos },
                },
                Tool::Eraser => {
                    let mut erased = Vec::new();
                    self.erase_along(pos, pos, &mut erased);
                    Gesture::Erase { last: pos, erased }
                }
                Tool::Text => {
                    let hit = self.text_at_doc(doc);
                    if hit.is_none() && self.layout.page_at_doc(doc).is_none() {
                        Gesture::Pan { last: pos }
                    } else {
                        Gesture::TextPress { origin: pos, hit }
                    }
                }
            },
            _ => return,
        };
        self.gesture = gesture;
        self.gesture_button = Some(button);
    }

    fn continue_gesture(&mut self, pos: Pos2) {
        let viewport = self.viewport;
        let mut g = std::mem::take(&mut self.gesture);
        match &mut g {
            Gesture::None | Gesture::Swallowed => {}
            Gesture::Pan { last } => {
                self.cam.pan(pos - *last);
                *last = pos;
            }
            Gesture::Rotate {
                last_angle, raw, ..
            } => {
                let a = (pos - viewport.center()).angle();
                *raw += wrap_angle(a - *last_angle);
                *last_angle = a;
                self.cam.rotation = snap_rotation(*raw, self.mods.shift);
            }
            Gesture::Draw(active) => {
                if pos.distance(active.last_screen) >= MIN_SAMPLE_PX {
                    let doc = self.cam.screen_to_doc(pos, viewport);
                    let n = self.layout.doc_to_norm(active.page, doc);
                    active
                        .points
                        .push([n[0].clamp(0.0, 1.0), n[1].clamp(0.0, 1.0)]);
                    active.last_screen = pos;
                }
            }
            Gesture::Erase { last, erased } => {
                let from = *last;
                *last = pos;
                self.erase_along(from, pos, erased);
            }
            Gesture::TextPress { origin, hit } => {
                if pos.distance(*origin) > DRAG_SLOP {
                    let origin = *origin;
                    match hit.and_then(|id| self.set.text_by_id(id).cloned()) {
                        Some(t) => {
                            let anchor = self.layout.norm_to_doc(t.page, t.pos);
                            let grab = self.cam.screen_to_doc(origin, viewport) - anchor;
                            g = Gesture::TextDrag {
                                id: t.id,
                                before: t,
                                grab,
                            };
                            self.gesture = g;
                            return self.continue_gesture(pos);
                        }
                        None => {
                            self.cam.pan(pos - origin);
                            g = Gesture::Pan { last: pos };
                        }
                    }
                }
            }
            Gesture::TextDrag { id, before, grab } => {
                let anchor = self.cam.screen_to_doc(pos, viewport) - *grab;
                let n = self.layout.doc_to_norm(before.page, anchor);
                self.set
                    .set_text_pos(*id, [n[0].clamp(0.0, 1.0), n[1].clamp(0.0, 1.0)]);
            }
        }
        self.gesture = g;
    }

    /// End the current gesture: `commit` applies it (release), otherwise it is undone
    /// (Escape, tool switch with nothing to keep).
    pub(super) fn finish_gesture(&mut self, commit: bool) {
        let g = std::mem::take(&mut self.gesture);
        self.gesture_button = None;
        match g {
            Gesture::None | Gesture::Swallowed | Gesture::Pan { .. } => {}
            Gesture::Rotate { start, .. } => {
                if !commit {
                    self.cam.rotation = start;
                }
            }
            Gesture::Draw(active) => {
                if commit {
                    self.commit_stroke(active);
                }
            }
            Gesture::Erase { erased, .. } => {
                if erased.is_empty() {
                    return;
                }
                if commit {
                    self.history.record_erase(erased);
                } else {
                    for (i, a) in erased.into_iter().rev() {
                        self.set.insert_at(i, a);
                    }
                }
                self.mark_dirty();
            }
            Gesture::TextPress { origin, hit } => {
                if !commit {
                    return;
                }
                let doc = self.cam.screen_to_doc(origin, self.viewport);
                match hit {
                    Some(id) => self.open_existing_text(id, Some(doc)),
                    None => {
                        if let Some((page, _)) = self.layout.page_at_doc(doc) {
                            self.open_new_text(page, doc);
                        }
                    }
                }
            }
            Gesture::TextDrag { id, before, .. } => {
                let Some(after) = self.set.text_by_id(id).cloned() else {
                    return;
                };
                if !commit {
                    self.set.replace_text(before);
                } else if after.pos != before.pos {
                    self.history.record_edit_text(before, after);
                    self.mark_dirty();
                }
            }
        }
    }

    pub(super) fn cancel_gesture(&mut self) {
        self.finish_gesture(false);
    }

    /// Wheel scroll / ctrl-wheel or pinch zoom / trackpad rotate, at the pointer.
    fn handle_wheel(&mut self) {
        let ctx = self.ctx.clone();
        let (scroll, zoom, rotate, hover) = ctx.input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.rotation_delta(),
                i.pointer.hover_pos(),
            )
        });
        let anchor = hover.unwrap_or(self.viewport.center());
        let mut moved = false;
        if (zoom - 1.0).abs() > f32::EPSILON {
            self.cam.zoom_around(zoom, anchor, self.viewport);
            self.fit = None;
            moved = true;
        } else if scroll != Vec2::ZERO {
            // Wheel moves through the page stack in doc space regardless of rotation, so
            // "down" always reads forward; negate so the gesture isn't inverted.
            self.cam.center -= scroll * SCROLL_SPEED / self.cam.zoom;
            moved = true;
        }
        if rotate != 0.0 {
            // Accumulate the unsnapped angle while the twist continues (like a rotate-drag),
            // or a slow twist near an axis would be snapped back every frame.
            let raw = match self.twist {
                Some((raw, at)) if self.now - at < TWIST_IDLE => raw + rotate,
                _ => self.cam.rotation + rotate,
            };
            self.twist = Some((raw, self.now));
            self.cam.rotation = snap_rotation(raw, self.mods.shift);
            moved = true;
        }
        if moved {
            self.glide = None;
        }
    }

    // --- drawing -------------------------------------------------------------------

    /// The end point of a straight line from `a` toward `b` on `page`, snapped onto the
    /// page's horizontal/vertical when nearly aligned (underlines, margins, boxes).
    pub(super) fn straight_end(&self, page: usize, a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
        let p = self.layout.pages[page];
        let d = Vec2::new((b[0] - a[0]) * p.w, (b[1] - a[1]) * p.h);
        if d.length() < f32::EPSILON {
            return b;
        }
        let angle = d.y.atan2(d.x);
        let nearest = (angle / FRAC_PI_2).round() * FRAC_PI_2;
        if (angle - nearest).abs() > LINE_SNAP {
            return b;
        }
        if (nearest / FRAC_PI_2).round() as i32 % 2 == 0 {
            [b[0], a[1]]
        } else {
            [a[0], b[1]]
        }
    }

    /// Build a finished stroke from the gesture and the current brush. A tap leaves a dot;
    /// Shift (held at release) makes a straight line; freehand is Chaikin-smoothed exactly
    /// like the live preview, then losslessly simplified.
    fn commit_stroke(&mut self, active: ActiveStroke) {
        let brush = if self.tool == Tool::Highlighter {
            self.highlighter
        } else {
            self.pen
        };
        let first = active.points[0];
        let last = *active.points.last().unwrap();
        let (kind, points) = if active.points.len() == 1 {
            (StrokeKind::Freehand, vec![first])
        } else if self.mods.shift {
            (
                StrokeKind::Line,
                vec![first, self.straight_end(active.page, first, last)],
            )
        } else {
            let p = self.layout.pages[active.page];
            let smooth = chaikin(&active.points, 2);
            let tolerance = SIMPLIFY_PX / self.cam.zoom;
            (
                StrokeKind::Freehand,
                simplify(&smooth, [p.w, p.h], tolerance),
            )
        };
        let stroke = Stroke {
            page: active.page,
            kind,
            points,
            color: brush.color.to_srgba_unmultiplied(),
            width_pt: brush.size,
            id: 0,
        };
        let id = self.set.add(stroke);
        // Record a clone carrying the assigned id for undo.
        if let Some(s) = self.set.strokes.iter().find(|s| s.id == id) {
            self.history.record_add(Annotation::Stroke(s.clone()));
        }
        self.mark_dirty();
    }

    // --- erasing -------------------------------------------------------------------

    /// Remove every annotation the eraser touches while moving from screen `a` to `b`
    /// (a swept segment, so fast drags don't skip marks).
    fn erase_along(&mut self, a: Pos2, b: Pos2, erased: &mut Vec<(usize, Annotation)>) {
        for id in self.hits_along(a, b) {
            if let Some(removed) = self.set.remove(id) {
                erased.push(removed);
            }
        }
    }

    /// Ids of annotations within eraser reach of the screen segment `a`–`b`, topmost (text,
    /// then latest strokes) first.
    pub(super) fn hits_along(&self, a: Pos2, b: Pos2) -> Vec<u64> {
        let (da, db) = (
            self.cam.screen_to_doc(a, self.viewport),
            self.cam.screen_to_doc(b, self.viewport),
        );
        let reach = self.eraser_px / self.cam.zoom;
        let mut hits = Vec::new();
        for t in self.set.texts.iter().rev() {
            if t.page < self.layout.len()
                && segment_rect_distance(da, db, self.text_bbox_doc(t)) <= reach
            {
                hits.push(t.id);
            }
        }
        for s in self.set.strokes.iter().rev() {
            if s.page >= self.layout.len() {
                continue;
            }
            let r = reach + s.width_pt * 0.5;
            if segment_rect_distance(da, db, self.stroke_bbox_doc(s)) > r {
                continue;
            }
            let pts: Vec<Pos2> = s
                .points
                .iter()
                .map(|n| self.layout.norm_to_doc(s.page, *n))
                .collect();
            let hit = if pts.len() == 1 {
                point_segment_distance(pts[0], da, db) <= r
            } else {
                pts.windows(2)
                    .any(|w| segment_distance(w[0], w[1], da, db) <= r)
            };
            if hit {
                hits.push(s.id);
            }
        }
        hits
    }

    /// Doc-space bounding box of a stroke's centreline (excluding its width).
    pub(super) fn stroke_bbox_doc(&self, s: &Stroke) -> Rect {
        let pts: Vec<Pos2> = s
            .points
            .iter()
            .map(|n| self.layout.norm_to_doc(s.page, *n))
            .collect();
        Rect::from_points(&pts)
    }
}

/// Shortest distance from point `p` to segment `a`–`b`.
fn point_segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Shortest distance between segments `a0`–`a1` and `b0`–`b1`.
fn segment_distance(a0: Pos2, a1: Pos2, b0: Pos2, b1: Pos2) -> f32 {
    if segments_cross(a0, a1, b0, b1) {
        return 0.0;
    }
    point_segment_distance(a0, b0, b1)
        .min(point_segment_distance(a1, b0, b1))
        .min(point_segment_distance(b0, a0, a1))
        .min(point_segment_distance(b1, a0, a1))
}

fn segments_cross(a0: Pos2, a1: Pos2, b0: Pos2, b1: Pos2) -> bool {
    let cross = |o: Pos2, p: Pos2, q: Pos2| (p - o).x * (q - o).y - (p - o).y * (q - o).x;
    let d1 = cross(b0, b1, a0);
    let d2 = cross(b0, b1, a1);
    let d3 = cross(a0, a1, b0);
    let d4 = cross(a0, a1, b1);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// Shortest distance from segment `a`–`b` to rectangle `r` (0 if they touch).
fn segment_rect_distance(a: Pos2, b: Pos2, r: Rect) -> f32 {
    if r.contains(a) || r.contains(b) {
        return 0.0;
    }
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ];
    (0..4)
        .map(|k| segment_distance(a, b, corners[k], corners[(k + 1) % 4]))
        .fold(f32::INFINITY, f32::min)
}
