//! Bookmark search: a palette over the document's outline. It opens on the section being
//! read; typing filters it, best match first (every word must match, see
//! [`OutlineQuery::filter`]); ↑ ↓ choose, and Enter or a click goes there.

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Frame, Id, Key, Margin, Modifiers, Order, Pos2,
    RichText, ScrollArea, Sense, Vec2,
};

use super::chrome::{keycap, pill};
use super::{App, TOOLBAR_SPACE};
use crate::icons::{self, Icon};
use crate::pdf::OutlineItem;
use crate::search::OutlineQuery;
use crate::theme;

/// Height of one bookmark row.
const ROW: f32 = 28.0;
/// Indentation per outline level (unfiltered list).
const INDENT: f32 = 14.0;
/// Deeper levels than this aren't indented further.
const MAX_INDENT_DEPTH: usize = 6;
/// Width of the page-number column.
const PAGE_COLUMN: f32 = 44.0;
/// The reading line, as a fraction of the way down the view: the section whose heading is
/// last above it is the one being read.
const READING_LINE: f32 = 0.33;

/// The open palette.
pub(super) struct Palette {
    query: String,
    /// The bookmarks listed (outline indices: in outline order, or best match first).
    shown: Vec<usize>,
    /// The highlighted row (an index into `shown`).
    selected: usize,
    /// The bookmark for the page being read, selected while the query is empty.
    home: usize,
    /// Scroll the selected row into view next frame.
    reveal: bool,
    /// The field still has to take keyboard focus.
    focus: bool,
    /// The list's scroll offset and visible height last frame.
    scroll: f32,
    list_height: f32,
}

impl Palette {
    /// Re-run the filter (the query or the outline changed).
    fn refilter(&mut self, outline: &[OutlineItem]) {
        let query = OutlineQuery::new(&self.query);
        self.shown = query.filter(outline);
        self.selected = if query.is_empty() {
            self.home.min(self.shown.len().saturating_sub(1))
        } else {
            0
        };
        self.reveal = true;
    }

    /// Move the selection by `delta` rows; single steps wrap around, page steps stop at the
    /// ends.
    fn move_selection(&mut self, delta: isize) {
        let n = self.shown.len() as isize;
        if n == 0 {
            return;
        }
        let at = self.selected as isize + delta;
        self.selected = if delta.abs() == 1 {
            at.rem_euclid(n)
        } else {
            at.clamp(0, n - 1)
        } as usize;
        self.reveal = true;
    }
}

impl App {
    /// `B`: open the bookmark palette (or close it), on the section being read.
    pub(super) fn toggle_bookmarks(&mut self) {
        if self.bookmarks.take().is_some() {
            return;
        }
        let Some(pdf) = self.pdf.as_ref() else {
            return;
        };
        if pdf.outline.is_empty() {
            self.notify("This PDF has no bookmarks");
            return;
        }
        // The section being read: the last bookmark that starts above the reading line.
        let (page, y) = self.reading_position(READING_LINE);
        let home = pdf
            .outline
            .iter()
            .rposition(|it| match it.dest {
                Some(d) if d.page == page => pdf.dest_offset(d).unwrap_or(0.0) <= y,
                Some(d) => d.page < page,
                None => false,
            })
            .unwrap_or(0);
        let mut palette = Palette {
            query: String::new(),
            shown: Vec::new(),
            selected: 0,
            home,
            reveal: true,
            focus: true,
            scroll: 0.0,
            list_height: 0.0,
        };
        palette.refilter(&pdf.outline);
        self.commit_text();
        self.help_open = false;
        self.bookmarks = Some(palette);
    }

    /// The outline changed (the file was reloaded): filter it again.
    pub(super) fn refilter_bookmarks(&mut self) {
        if let (Some(palette), Some(pdf)) = (self.bookmarks.as_mut(), self.pdf.as_ref()) {
            palette.refilter(&pdf.outline);
        }
    }

    /// Go to bookmark `index`.
    fn follow_bookmark(&mut self, index: usize) {
        let Some(pdf) = self.pdf.as_ref() else {
            return;
        };
        let Some(dest) = pdf.outline.get(index).and_then(|it| it.dest) else {
            self.notify("That bookmark points outside this document");
            return;
        };
        if dest.page >= self.layout.len() {
            return;
        }
        let offset = pdf.dest_offset(dest);
        self.bookmarks = None;
        self.show_destination(dest.page, offset);
    }

    pub(super) fn bookmarks_palette(&mut self, ctx: &egui::Context) {
        let (Some(palette), Some(pdf)) = (self.bookmarks.as_mut(), self.pdf.as_ref()) else {
            return;
        };
        let outline = &pdf.outline;
        let screen = self.viewport;
        let width = (screen.width() - 32.0).clamp(240.0, 600.0);
        let top = if self.chrome_visible {
            TOOLBAR_SPACE
        } else {
            theme::MARGIN * 3.0
        };
        let max_list = (screen.height() - top - 140.0).clamp(ROW * 3.0, ROW * 14.0);
        let mut close = false;
        let mut follow = None;

        egui::Area::new(Id::new("bookmarks_backdrop"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, Sense::CLICK);
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(110));
                close = resp.clicked();
            });

        egui::Area::new(Id::new("bookmarks_palette"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, top))
            .show(ctx, |ui| {
                pill()
                    .corner_radius(14)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        // List keys, taken before the field sees them (it would move its
                        // caret).
                        let keys = ui.input_mut(|i| {
                            [
                                Key::ArrowUp,
                                Key::ArrowDown,
                                Key::PageUp,
                                Key::PageDown,
                                Key::Enter,
                            ]
                            .map(|k| i.consume_key(Modifiers::NONE, k))
                        });
                        let page_rows = ((palette.list_height / ROW) as isize - 1).max(1);
                        for (pressed, delta) in keys[..4].iter().zip([-1, 1, -page_rows, page_rows])
                        {
                            if *pressed {
                                palette.move_selection(delta);
                            }
                        }
                        if keys[4] {
                            follow = palette.shown.get(palette.selected).copied();
                        }

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::new(26.0, ROW), Sense::hover());
                            icons::paint(
                                ui.painter(),
                                r.shrink(5.0),
                                Icon::Bookmark,
                                theme::TEXT_DIM,
                            );
                            let count = if palette.query.trim().is_empty() {
                                format!("{}", outline.len())
                            } else {
                                format!("{} of {}", palette.shown.len(), outline.len())
                            };
                            let count_galley = ui.painter().layout_no_wrap(
                                count,
                                FontId::proportional(12.5),
                                theme::TEXT_FAINT,
                            );
                            let field_width = ui.available_width() - count_galley.size().x - 12.0;
                            let field = egui::TextEdit::singleline(&mut palette.query)
                                .id(Id::new("nullpdf_bookmark_field"))
                                .hint_text(
                                    RichText::new("Search bookmarks").color(theme::TEXT_FAINT),
                                )
                                .font(FontId::proportional(15.0))
                                .text_color(theme::TEXT)
                                .frame(Frame::NONE)
                                .margin(Margin::ZERO)
                                .desired_width(field_width)
                                .min_size(Vec2::new(field_width, ROW))
                                .vertical_align(Align::Center)
                                .return_key(None)
                                .lock_focus(true)
                                .show(ui);
                            let resp = field.response.response;
                            if palette.focus {
                                resp.request_focus();
                                let mut state = field.state;
                                state
                                    .cursor
                                    .set_char_range(Some(CCursorRange::one(CCursor::new(
                                        palette.query.chars().count(),
                                    ))));
                                state.store(ui.ctx(), resp.id);
                                palette.focus = false;
                            }
                            if resp.changed() {
                                palette.refilter(outline);
                            }
                            let (r, _) = ui.allocate_exact_size(
                                Vec2::new(count_galley.size().x + 8.0, ROW),
                                Sense::hover(),
                            );
                            ui.painter().galley(
                                Pos2::new(r.left(), r.center().y - count_galley.size().y * 0.5),
                                count_galley,
                                theme::TEXT_FAINT,
                            );
                        });

                        let (r, _) = ui.allocate_exact_size(Vec2::new(width, 9.0), Sense::hover());
                        ui.painter().hline(
                            r.x_range(),
                            r.center().y,
                            theme::line(1.0, theme::BORDER),
                        );

                        if palette.shown.is_empty() {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("No bookmarks match")
                                    .size(13.0)
                                    .color(theme::TEXT_DIM),
                            );
                            ui.add_space(6.0);
                        } else {
                            let clicked = ui
                                .scope(|ui| bookmark_list(ui, palette, outline, max_list))
                                .inner;
                            follow = follow.or(clicked);
                        }

                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let hint = |ui: &mut egui::Ui, s: &str| {
                                ui.label(RichText::new(s).size(12.0).color(theme::TEXT_FAINT));
                            };
                            keycap(ui, "↑");
                            keycap(ui, "↓");
                            hint(ui, "choose   ");
                            keycap(ui, "Enter");
                            hint(ui, "go   ");
                            keycap(ui, "Esc");
                            hint(ui, "close");
                        });
                    });
            });

        if let Some(index) = follow {
            self.follow_bookmark(index);
        } else if close {
            self.bookmarks = None;
        }
    }
}

/// The scrolling list of bookmarks; returns the one clicked, if any.
fn bookmark_list(
    ui: &mut egui::Ui,
    palette: &mut Palette,
    outline: &[OutlineItem],
    max_height: f32,
) -> Option<usize> {
    let query = OutlineQuery::new(&palette.query);
    let filtered = !query.is_empty();
    let mut area = ScrollArea::vertical()
        .id_salt("bookmark_list")
        .max_height(max_height)
        .auto_shrink([false, true]);
    if palette.reveal {
        // Keep the selection in view; on opening, put it a third of the way down.
        let visible = if palette.list_height > 0.0 {
            palette.list_height
        } else {
            max_height
        };
        let row_top = palette.selected as f32 * ROW;
        let offset = if palette.list_height <= 0.0 {
            row_top - visible / 3.0
        } else if row_top < palette.scroll {
            row_top
        } else if row_top + ROW > palette.scroll + visible {
            row_top + ROW - visible
        } else {
            palette.scroll
        };
        area = area.vertical_scroll_offset(offset.max(0.0));
        palette.reveal = false;
    }
    let mut clicked = None;
    // Rows sit flush: `show_rows` spaces them by the item spacing it's called with.
    ui.spacing_mut().item_spacing.y = 0.0;
    let out = area.show_rows(ui, ROW, palette.shown.len(), |ui, rows| {
        for row in rows {
            let index = palette.shown[row];
            let item = &outline[index];
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::CLICK);
            let selected = row == palette.selected;
            let painter = ui.painter();
            if selected {
                painter.rect_filled(rect, 7.0, theme::SURFACE_ACTIVE);
            } else if resp.hovered() {
                painter.rect_filled(rect, 7.0, theme::SURFACE_HOVER);
            }
            let indent = if filtered {
                0.0
            } else {
                item.depth.min(MAX_INDENT_DEPTH) as f32 * INDENT
            };
            let text_left = rect.left() + 10.0 + indent;
            let text_width = (rect.right() - PAGE_COLUMN - text_left).max(20.0);
            let job = row_text(item, outline, &query, filtered, text_width);
            let galley = painter.layout_job(job);
            painter.galley(
                Pos2::new(text_left, rect.center().y - galley.size().y * 0.5),
                galley,
                theme::TEXT,
            );
            let page = item
                .dest
                .map_or_else(|| "—".to_owned(), |d| (d.page + 1).to_string());
            painter.text(
                Pos2::new(rect.right() - 10.0, rect.center().y),
                Align2::RIGHT_CENTER,
                page,
                FontId::proportional(12.5),
                if selected {
                    theme::TEXT_DIM
                } else {
                    theme::TEXT_FAINT
                },
            );
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                clicked = Some(index);
            }
        }
    });
    palette.scroll = out.state.offset.y;
    palette.list_height = out.inner_rect.height();
    clicked
}

/// A row's text: the title with matching parts highlighted and, when filtering, the
/// enclosing bookmarks after it (faint), elided to `width`.
fn row_text(
    item: &OutlineItem,
    outline: &[OutlineItem],
    query: &OutlineQuery,
    filtered: bool,
    width: f32,
) -> LayoutJob {
    let font = FontId::proportional(13.5);
    let plain = TextFormat::simple(
        font.clone(),
        if item.dest.is_some() {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    let hit = TextFormat::simple(font.clone(), theme::ACCENT_TEXT);
    let mut job = LayoutJob {
        wrap: TextWrapping {
            max_width: width,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        },
        ..Default::default()
    };
    let title = item.title.as_str();
    let mut at = 0;
    for r in query.highlights(title) {
        job.append(&title[at..r.start], 0.0, plain.clone());
        job.append(&title[r.clone()], 0.0, hit.clone());
        at = r.end;
    }
    job.append(&title[at..], 0.0, plain);
    if filtered {
        let mut path = Vec::new();
        let mut parent = item.parent;
        while let Some(p) = parent {
            path.push(outline[p].title.as_str());
            parent = outline[p].parent;
        }
        if !path.is_empty() {
            path.reverse();
            job.append(
                &path.join(" › "),
                12.0,
                TextFormat::simple(FontId::proportional(12.5), theme::TEXT_FAINT),
            );
        }
    }
    job
}
