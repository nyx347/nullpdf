//! Find in document: the find bar, the incremental search behind it, and moving between
//! matches.
//!
//! The bar floats at the bottom right, in the strip the status line already keeps clear.
//! Typing searches as you go, from the reading position; Enter / Shift+Enter (or `N` /
//! Shift+N once the field is left) step through the matches, gliding each into view.

use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange};
use egui::{
    Align, Align2, Color32, CursorIcon, Event, FontId, Frame, Id, Key, Margin, Order, Pos2, Rect,
    Response, RichText, Sense, Ui, Vec2,
};

use super::chrome::{icon_button_sized, pill, tooltip};
use super::App;
use crate::icons::{self, Icon};
use crate::keymap::Action;
use crate::search::TextSearch;
use crate::theme;

/// Search time per frame; the remaining pages wait for the next frame.
const SEARCH_BUDGET: Duration = Duration::from_millis(8);
/// A match closer than this (screen px) to the edge of the visible area is brought into view.
const REVEAL_MARGIN: f32 = 32.0;
/// Height of the find bar's controls; with the pill's padding the bar is exactly as tall as
/// the strip kept clear for the status line.
const ROW: f32 = 22.0;
/// Width of the match count ("12 / 345", "No matches").
const COUNT_WIDTH: f32 = 76.0;

/// Id of the find field (to give it focus and pre-select its text).
fn find_field_id() -> Id {
    Id::new("nullpdf_find_field")
}

impl App {
    /// `/` or Ctrl+F: open the find bar, or return to it, with the query selected so typing
    /// replaces it.
    pub(super) fn open_find(&mut self) {
        self.commit_text();
        self.bookmarks = None;
        self.find_open = true;
        self.find_focus = true;
        if self.search.is_none() {
            self.restart_search(false);
        }
    }

    /// Close the find bar and drop its highlights; the query is kept for next time.
    pub(super) fn close_find(&mut self) {
        self.find_open = false;
        self.search = None;
        self.blur_find();
    }

    /// Take keyboard focus from the find field (the bar and its matches stay), so
    /// single-key shortcuts work again.
    pub(super) fn blur_find(&self) {
        self.ctx.memory_mut(|m| m.surrender_focus(find_field_id()));
    }

    /// The next (`dir > 0`) or previous match. With the bar closed, the last search is run
    /// again from the reading position.
    pub(super) fn find_step(&mut self, dir: i32) {
        let Some(search) = self.search.as_mut() else {
            if self.find_query.trim().is_empty() {
                self.open_find();
            } else {
                self.find_open = true;
                self.restart_search(dir < 0);
            }
            return;
        };
        if search.step(dir) {
            self.reveal_match();
        }
    }

    /// Search afresh for the query, from the current match if there is one (so refining the
    /// query stays put), else from the top of the view. `backward` starts at the last match
    /// before that point instead of the first one after it.
    pub(super) fn restart_search(&mut self, backward: bool) {
        let anchor = self
            .current_match_anchor()
            .unwrap_or_else(|| self.reading_position(0.0));
        self.search = None;
        if self.find_query.trim().is_empty() || self.layout.is_empty() {
            return;
        }
        let mut search = TextSearch::new(
            self.find_query.clone(),
            self.find_case,
            self.layout.len(),
            anchor,
        );
        if backward {
            search.start_backward();
        }
        self.search = Some(search);
    }

    /// Search more pages, within this frame's budget. Returns `true` while pages remain.
    pub(super) fn search_tick(&mut self) -> bool {
        let (Some(pdf), Some(search)) = (self.pdf.as_ref(), self.search.as_mut()) else {
            return false;
        };
        let start = Instant::now();
        let mut moved = false;
        while let Some(page) = search.next_page() {
            let hits = pdf.find_text(page, &search.query, search.match_case);
            moved |= search.add_page(page, hits);
            if start.elapsed() > SEARCH_BUDGET {
                break;
            }
        }
        let pending = !search.is_complete();
        if moved {
            self.reveal_match();
        }
        pending
    }

    /// The current match's position as a search anchor: its page, and its top.
    fn current_match_anchor(&self) -> Option<(usize, f32)> {
        let search = self.search.as_ref()?;
        let (page, i) = search.current?;
        Some((page, search.page(page)?.bbox(i).top()))
    }

    /// Doc-space bounding box of the current match.
    pub(super) fn current_match_doc(&self) -> Option<Rect> {
        let search = self.search.as_ref()?;
        let (page, i) = search.current?;
        if page >= self.layout.len() {
            return None;
        }
        let offset = self.layout.page_rect(page).min.to_vec2();
        Some(search.page(page)?.bbox(i).translate(offset))
    }

    /// Glide the current match into full view if it isn't: centred vertically, and
    /// horizontally too when it's off to the side.
    fn reveal_match(&mut self) {
        let Some(bbox) = self.current_match_doc() else {
            return;
        };
        let cam = self.glide_target();
        let safe = self.safe_rect();
        let margin = REVEAL_MARGIN
            .min(safe.width() * 0.25)
            .min(safe.height() * 0.25);
        let inner = safe.shrink(margin);
        let corners = [
            bbox.left_top(),
            bbox.right_top(),
            bbox.right_bottom(),
            bbox.left_bottom(),
        ]
        .map(|p| cam.doc_to_screen(p, self.viewport));
        let shown = Rect::from_points(&corners);
        if inner.contains_rect(shown) {
            return;
        }
        let x = if shown.left() >= inner.left() && shown.right() <= inner.right() {
            shown.center().x
        } else {
            safe.center().x
        };
        let mut target = cam;
        target.center = cam.center_for(bbox.center(), Pos2::new(x, safe.center().y), self.viewport);
        self.glide_to(target);
    }

    /// The match count, or how the search is going.
    fn find_status(&self) -> (String, Color32) {
        match &self.search {
            None => (String::new(), theme::TEXT_DIM),
            Some(s) if !s.is_complete() => match s.total() {
                0 => ("…".to_owned(), theme::TEXT_FAINT),
                n => (format!("{n}…"), theme::TEXT_FAINT),
            },
            Some(s) if s.total() == 0 => ("No matches".to_owned(), theme::DANGER),
            Some(s) => (
                format!("{} / {}", s.position().unwrap_or(0), s.total()),
                theme::TEXT_DIM,
            ),
        }
    }

    /// The find bar: query field, match count, previous / next, match case, close.
    pub(super) fn find_bar(&mut self, ctx: &egui::Context) {
        if !self.find_open || self.pdf.is_none() {
            return;
        }
        let field_width = (self.viewport.width() * 0.22).clamp(120.0, 220.0);
        let (status, status_color) = self.find_status();
        let any = self.search.as_ref().is_some_and(|s| s.total() > 0);
        let (mut edited, mut step, mut close) = (false, 0, false);
        egui::Area::new(Id::new("find_bar"))
            .order(Order::Foreground)
            .anchor(
                Align2::RIGHT_BOTTOM,
                Vec2::new(-theme::MARGIN, -theme::MARGIN),
            )
            .show(ctx, |ui| {
                pill().inner_margin(Margin::same(3)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::new(ROW + 4.0, ROW), Sense::hover());
                        icons::paint(ui.painter(), r.shrink(4.0), Icon::Search, theme::TEXT_DIM);

                        if self.find_focus {
                            // Select the whole query, so typing replaces it.
                            let id = find_field_id();
                            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id)
                                .unwrap_or_default();
                            let end = CCursor::new(self.find_query.chars().count());
                            state
                                .cursor
                                .set_char_range(Some(CCursorRange::two(CCursor::new(0), end)));
                            state.store(ui.ctx(), id);
                        }
                        let field = egui::TextEdit::singleline(&mut self.find_query)
                            .id(find_field_id())
                            .hint_text(RichText::new("Find in document").color(theme::TEXT_FAINT))
                            .font(FontId::proportional(13.5))
                            .text_color(theme::TEXT)
                            .frame(Frame::NONE)
                            .margin(Margin::symmetric(2, 0))
                            .desired_width(field_width)
                            .min_size(Vec2::new(field_width, ROW))
                            .vertical_align(Align::Center)
                            // Enter steps through matches; it mustn't end the edit.
                            .return_key(None)
                            .lock_focus(true)
                            .show(ui);
                        let resp = field.response.response;
                        if self.find_focus {
                            resp.request_focus();
                            self.find_focus = false;
                        }
                        edited = resp.changed();
                        if resp.has_focus() {
                            step += enter_steps(ui);
                        }

                        let (r, _) =
                            ui.allocate_exact_size(Vec2::new(COUNT_WIDTH, ROW), Sense::hover());
                        ui.painter().text(
                            r.right_center() - Vec2::new(6.0, 0.0),
                            Align2::RIGHT_CENTER,
                            status,
                            FontId::proportional(12.5),
                            status_color,
                        );

                        let r = icon_button_sized(ui, Icon::Up, ROW, false, any);
                        tooltip(&r, "Previous match", &[Action::FindPrev]);
                        if r.clicked() {
                            step = -1;
                        }
                        let r = icon_button_sized(ui, Icon::Down, ROW, false, any);
                        tooltip(&r, "Next match", &[Action::FindNext]);
                        if r.clicked() {
                            step = 1;
                        }
                        let r = case_toggle(ui, self.find_case);
                        tooltip(&r, "Match case", &[]);
                        if r.clicked() {
                            self.find_case = !self.find_case;
                            edited = true;
                        }
                        let r = icon_button_sized(ui, Icon::Close, ROW, false, true);
                        tooltip(&r, "Close", &[Action::Cancel]);
                        close = r.clicked();
                    });
                });
            });
        if close {
            self.close_find();
            return;
        }
        if edited {
            self.restart_search(false);
        }
        for _ in 0..step.abs() {
            self.find_step(step.signum());
        }
    }
}

/// This frame's Enter presses as steps: +1 (next) each, −1 (previous) with Shift. Shift is
/// read from each press itself: it's often let go of in the same frame.
fn enter_steps(ui: &Ui) -> i32 {
    ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.shift => -1,
                Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    ..
                } => 1,
                _ => 0,
            })
            .sum()
    })
}

/// The "Aa" match-case toggle.
fn case_toggle(ui: &mut Ui, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ROW + 6.0, ROW), Sense::CLICK);
    let bg = if on {
        theme::SURFACE_ACTIVE
    } else if resp.hovered() {
        theme::SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 6.0, bg);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Aa",
        FontId::proportional(12.5),
        if on { Color32::WHITE } else { theme::TEXT_DIM },
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}
