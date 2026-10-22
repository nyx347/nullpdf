//! Floating chrome: the toolbar pill, status line, toasts, the `?` cheat sheet, the welcome
//! screen and the drop overlay. (The find bar and the bookmark palette live in `find` and
//! `bookmarks`, built from the same widgets.)
//!
//! Every control is a custom-painted, *non-focusable* widget (`Sense::CLICK`), so clicking
//! the chrome can never leave a widget holding keyboard focus — which would otherwise make
//! egui swallow the single-key shortcuts.

use egui::{
    Align2, Color32, CursorIcon, FontId, Frame, Id, Margin, Order, Pos2, Rect, Response, RichText,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use super::{format_size, App, ACTUAL_SIZE_ZOOM, TOAST_FADE_IN, TOAST_FADE_OUT};
use crate::icons::{self, Icon};
use crate::keymap::{self, Action, HELP, MOUSE_HELP};
use crate::theme;
use crate::tools::{Tool, PALETTE};

const TOOLS: [(Tool, Icon); 5] = [
    (Tool::Nav, Icon::Hand),
    (Tool::Pen, Icon::Pen),
    (Tool::Highlighter, Icon::Highlighter),
    (Tool::Eraser, Icon::Eraser),
    (Tool::Text, Icon::Text),
];

/// Below this window width the toolbar drops its swatches, find and file buttons (the custom
/// colour button and the keys still reach everything).
const COMPACT_WIDTH: f32 = 840.0;

/// Wheel distance (px) that steps the size control by one rung.
const SIZE_WHEEL_STEP: f32 = 40.0;

pub(super) fn pill() -> Frame {
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(theme::line(1.0, theme::BORDER))
        .corner_radius(theme::PILL_RADIUS)
        .shadow(theme::shadow())
        .inner_margin(Margin::same(4))
}

fn same_rgb(a: Color32, b: Color32) -> bool {
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    a[..3] == b[..3]
}

fn opaque(c: Color32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}

/// Square icon button; `active` marks the selected tool.
fn icon_button(ui: &mut Ui, icon: Icon, active: bool, enabled: bool) -> Response {
    icon_button_sized(ui, icon, theme::BUTTON, active, enabled)
}

/// [`icon_button`] of any size.
pub(super) fn icon_button_sized(
    ui: &mut Ui,
    icon: Icon,
    side: f32,
    active: bool,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::CLICK
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), sense);
    let painter = ui.painter();
    let bg = if active || (enabled && resp.is_pointer_button_down_on()) {
        theme::SURFACE_ACTIVE
    } else if enabled && resp.hovered() {
        theme::SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, side * 0.25, bg);
    let fg = match (enabled, active) {
        (false, _) => theme::TEXT_FAINT,
        (true, true) => Color32::WHITE,
        (true, false) => theme::TEXT,
    };
    icons::paint(painter, rect.shrink(side * 0.1875), icon, fg);
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, theme::BUTTON), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(8.0),
        theme::line(1.0, theme::BORDER),
    );
}

/// A keyboard key, drawn as a small cap. Arrow keys get painted arrows (the UI font has
/// no arrow glyphs).
pub(super) fn keycap(ui: &mut Ui, label: &str) {
    let font = FontId::proportional(12.0);
    let arrow = match label {
        "↑" => Some(-Vec2::Y),
        "↓" => Some(Vec2::Y),
        "←" => Some(-Vec2::X),
        "→" => Some(Vec2::X),
        _ => None,
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, theme::TEXT);
    let inner = if arrow.is_some() {
        10.0
    } else {
        galley.size().x
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(inner.max(10.0) + 12.0, 20.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        5.0,
        theme::SURFACE_HOVER,
        theme::line(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    // A slightly heavier bottom edge reads as a physical key.
    painter.hline(
        rect.x_range().shrink(3.0),
        rect.bottom() - 1.0,
        theme::line(1.0, Color32::from_black_alpha(90)),
    );
    match arrow {
        Some(d) => {
            let c = rect.center();
            let side = Vec2::new(-d.y, d.x);
            let tip = c + d * 4.0;
            let base = c - d * 3.0;
            painter.add(Shape::convex_polygon(
                vec![tip, base + side * 4.0, base - side * 4.0],
                theme::TEXT,
                Stroke::NONE,
            ));
        }
        None => {
            painter.galley(rect.center() - galley.size() * 0.5, galley, theme::TEXT);
        }
    }
}

/// Keycaps for one action's primary binding, e.g. `Shift` `Z`.
fn action_caps(ui: &mut Ui, action: Action) {
    ui.spacing_mut().item_spacing.x = 3.0;
    for cap in keymap::primary_caps(action) {
        keycap(ui, cap);
    }
}

/// Hover tooltip: what the control does and its keys.
pub(super) fn tooltip(resp: &Response, label: &str, actions: &[Action]) {
    resp.clone().on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(theme::TEXT));
            ui.add_space(4.0);
            for a in actions {
                action_caps(ui, *a);
                ui.add_space(3.0);
            }
        });
    });
}

impl App {
    pub(super) fn chrome(&mut self, actions: &mut Vec<Action>) {
        let ctx = self.ctx.clone();
        if self.pdf.is_none() {
            self.welcome(&ctx, actions);
        } else if self.chrome_visible {
            self.toolbar(&ctx, actions);
            self.status_line(&ctx);
        }
        self.find_bar(&ctx);
        self.bookmarks_palette(&ctx);
        self.toast_ui(&ctx);
        if self.help_open {
            self.help_sheet(&ctx, actions);
        }
        self.drop_overlay(&ctx);
    }

    fn toolbar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let compact = self.viewport.width() < COMPACT_WIDTH;
        egui::Area::new(Id::new("toolbar"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, theme::MARGIN))
            .show(ctx, |ui| {
                pill().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                        for (tool, icon) in TOOLS {
                            let action = if tool == Tool::Eraser {
                                Action::ToggleEraser
                            } else {
                                Action::Tool(tool)
                            };
                            let r = icon_button(ui, icon, self.tool == tool, true);
                            tooltip(&r, tool.name(), &[action]);
                            if r.clicked() {
                                actions.push(action);
                            }
                        }
                        divider(ui);
                        if !compact {
                            let current = self.current_color();
                            for (i, (name, c)) in PALETTE.iter().enumerate() {
                                let r = self.swatch(ui, *c, same_rgb(current, *c));
                                tooltip(&r, name, &[Action::Color(i)]);
                                if r.clicked() {
                                    actions.push(Action::Color(i));
                                }
                            }
                        }
                        self.custom_color(ui, compact);
                        divider(ui);
                        self.size_control(ui);
                        divider(ui);
                        let r = icon_button(ui, Icon::Undo, false, self.history.can_undo());
                        tooltip(&r, "Undo", &[Action::Undo]);
                        if r.clicked() {
                            actions.push(Action::Undo);
                        }
                        let r = icon_button(ui, Icon::Redo, false, self.history.can_redo());
                        tooltip(&r, "Redo", &[Action::Redo]);
                        if r.clicked() {
                            actions.push(Action::Redo);
                        }
                        divider(ui);
                        let mut file = vec![(Icon::Help, "Shortcuts", Action::Help)];
                        if !compact {
                            file.splice(
                                0..0,
                                [
                                    (Icon::Search, "Find in document", Action::Find),
                                    (Icon::Bookmark, "Search bookmarks", Action::Bookmarks),
                                    (Icon::Open, "Open", Action::Open),
                                    (Icon::Export, "Export flattened copy", Action::Export),
                                ],
                            );
                        }
                        let has_outline = self.pdf.as_ref().is_some_and(|p| !p.outline.is_empty());
                        for (icon, label, action) in file {
                            let active = match action {
                                Action::Help => self.help_open,
                                Action::Find => self.find_open,
                                Action::Bookmarks => self.bookmarks.is_some(),
                                _ => false,
                            };
                            let enabled = action != Action::Bookmarks || has_outline;
                            let r = icon_button(ui, icon, active, enabled);
                            let label = if enabled {
                                label
                            } else {
                                "No bookmarks in this PDF"
                            };
                            tooltip(&r, label, &[action]);
                            if r.clicked() {
                                actions.push(action);
                            }
                        }
                    });
                });
            });
    }

    fn swatch(&self, ui: &mut Ui, color: Color32, selected: bool) -> Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(22.0, theme::BUTTON), Sense::CLICK);
        let c = rect.center();
        let painter = ui.painter();
        if selected {
            painter.circle_stroke(c, 10.0, theme::line(2.0, theme::TEXT));
        } else if resp.hovered() {
            painter.circle_stroke(c, 10.0, theme::line(1.0, theme::TEXT_DIM));
        }
        painter.circle_filled(c, 7.0, color);
        painter.circle_stroke(c, 7.0, theme::line(1.0, Color32::from_white_alpha(36)));
        resp.on_hover_cursor(CursorIcon::PointingHand)
    }

    /// Hue-ring button opening a full colour picker (with opacity) for the current target.
    /// Shows the current colour in its centre when it isn't one of the swatches.
    fn custom_color(&mut self, ui: &mut Ui, compact: bool) {
        let current = self.current_color();
        let custom = !PALETTE.iter().any(|(_, c)| same_rgb(current, *c));
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(28.0, theme::BUTTON), Sense::CLICK);
        let c = rect.center();
        let painter = ui.painter();
        if custom {
            painter.circle_stroke(c, 12.0, theme::line(2.0, theme::TEXT));
        } else if resp.hovered() {
            painter.circle_stroke(c, 12.0, theme::line(1.0, theme::TEXT_DIM));
        }
        icons::paint_hue_ring(painter, c, 9.0, 5.5);
        if custom || compact {
            painter.circle_filled(c, 4.5, opaque(current));
        }
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        tooltip(&resp, "Custom colour & opacity", &[]);

        let mut color = current;
        let mut changed = false;
        egui::Popup::from_toggle_button_response(&resp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                // Stock picker, sized to match its row of channel values.
                ui.spacing_mut().slider_width = 216.0;
                if compact {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (_, c) in PALETTE {
                            if self.swatch(ui, c, same_rgb(color, c)).clicked() {
                                color = c;
                                changed = true;
                            }
                        }
                    });
                }
                changed |= egui::color_picker::color_picker_color32(
                    ui,
                    &mut color,
                    egui::color_picker::Alpha::OnlyBlend,
                );
            });
        if changed {
            self.set_color(color, false);
        }
    }

    /// Compact log-scale slider for the current size (pen/highlighter width, font size or
    /// eraser radius). Drag or click to set; the wheel steps the same rungs as `[` / `]`.
    fn size_control(&mut self, ui: &mut Ui) {
        let (_, ladder, unit) = self.current_size();
        let (lo, hi) = (ladder[0], ladder[ladder.len() - 1]);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(116.0, theme::BUTTON), Sense::CLICK | Sense::DRAG);
        let track = Rect::from_min_max(
            Pos2::new(rect.left() + 10.0, rect.center().y - 2.0),
            Pos2::new(rect.left() + 70.0, rect.center().y + 2.0),
        );
        let to_t = |v: f32| ((v.ln() - lo.ln()) / (hi.ln() - lo.ln())).clamp(0.0, 1.0);

        if let Some(p) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on() {
                let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
                let raw = (lo.ln() + t * (hi.ln() - lo.ln())).exp();
                let step = if unit == "px" || raw >= 12.0 {
                    1.0
                } else if raw >= 4.0 {
                    0.5
                } else {
                    0.25
                };
                self.set_size(((raw / step).round() * step).clamp(lo, hi));
            }
        }
        if resp.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            self.size_wheel += dy;
            while self.size_wheel.abs() >= SIZE_WHEEL_STEP {
                let dir = self.size_wheel.signum() as i32;
                self.size_wheel -= dir as f32 * SIZE_WHEEL_STEP;
                let (v, ladder, _) = self.current_size();
                self.set_size(crate::tools::step_size(ladder, v, dir));
            }
        } else {
            self.size_wheel = 0.0;
        }

        let (value, _, _) = self.current_size();
        let painter = ui.painter();
        let x = track.left() + to_t(value) * track.width();
        painter.rect_filled(track, 2.0, theme::BORDER);
        painter.rect_filled(
            Rect::from_min_max(track.min, Pos2::new(x, track.max.y)),
            2.0,
            theme::TEXT_DIM,
        );
        let hot = resp.hovered() || resp.dragged();
        painter.circle_filled(
            Pos2::new(x, track.center().y),
            if hot { 7.0 } else { 6.0 },
            theme::TEXT,
        );
        painter.text(
            Pos2::new(track.right() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!("{} {unit}", format_size(value)),
            FontId::proportional(12.5),
            theme::TEXT_DIM,
        );
        let resp = resp.on_hover_cursor(CursorIcon::ResizeHorizontal);
        tooltip(&resp, "Size", &[Action::SizeDown, Action::SizeUp]);
    }

    /// File · page · zoom (· rotation) in a quiet pill at the bottom left. The find bar
    /// shares the row, so while it's open the file name shortens or goes (and on a very
    /// narrow window the whole line does).
    fn status_line(&mut self, ctx: &egui::Context) {
        let width = self.viewport.width();
        let name_chars = match (self.find_open, width) {
            (false, _) => 42,
            (true, w) if w >= 1040.0 => 30,
            (true, w) if w >= 860.0 => 14,
            (true, w) if w >= 620.0 => 0,
            (true, _) => return,
        };
        let page = self.current_page() + 1;
        let pages = self.layout.len();
        let zoom = self.cam.zoom / ACTUAL_SIZE_ZOOM * 100.0;
        let rotation = crate::view::wrap_angle(self.cam.rotation).to_degrees();
        let name = elide(&self.file_name(), name_chars);
        let mut upright = false;
        egui::Area::new(Id::new("status"))
            .order(Order::Foreground)
            .anchor(
                Align2::LEFT_BOTTOM,
                Vec2::new(theme::MARGIN, -theme::MARGIN),
            )
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(theme::line(1.0, theme::BORDER))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let text = |s: String, c: Color32| RichText::new(s).size(12.5).color(c);
                            let dot = |ui: &mut Ui| {
                                ui.label(text("  ·  ".into(), theme::TEXT_FAINT));
                            };
                            if !name.is_empty() {
                                ui.label(text(name, theme::TEXT_DIM));
                                dot(ui);
                            }
                            ui.label(text(format!("{page} / {pages}"), theme::TEXT));
                            dot(ui);
                            ui.label(text(format!("{zoom:.0}%"), theme::TEXT_DIM));
                            if self.cam.rotation != 0.0 {
                                dot(ui);
                                let (rect, resp) =
                                    ui.allocate_exact_size(Vec2::new(54.0, 18.0), Sense::CLICK);
                                let hot = resp.hovered();
                                if hot {
                                    ui.painter().rect_filled(rect, 5.0, theme::SURFACE_HOVER);
                                }
                                let col = if hot { theme::TEXT } else { theme::TEXT_DIM };
                                icons::paint(
                                    ui.painter(),
                                    Rect::from_center_size(
                                        rect.left_center() + Vec2::new(9.0, 0.0),
                                        Vec2::splat(13.0),
                                    ),
                                    Icon::Rotate,
                                    col,
                                );
                                ui.painter().text(
                                    rect.left_center() + Vec2::new(19.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    format!("{rotation:.0}°"),
                                    FontId::proportional(12.5),
                                    col,
                                );
                                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                                tooltip(&resp, "Make upright", &[]);
                                upright = resp.clicked();
                            }
                        });
                    });
            });
        if upright {
            let mut cam = self.glide_target();
            cam.rotation = 0.0;
            self.glide_to(cam);
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let age = self.now - toast.born;
        let ttl = toast.ttl();
        if age > ttl {
            self.toast = None;
            return;
        }
        let alpha =
            ((age / TOAST_FADE_IN).min(1.0) * ((ttl - age) / TOAST_FADE_OUT).min(1.0)) as f32;
        let lift = if (self.chrome_visible || self.find_open) && self.pdf.is_some() {
            theme::MARGIN + 40.0
        } else {
            theme::MARGIN
        };
        let (text, error) = (toast.text.clone(), toast.error);
        egui::Area::new(Id::new("toast"))
            .order(Order::Tooltip)
            .interactable(false)
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -lift))
            .show(ctx, |ui| {
                ui.multiply_opacity(alpha);
                pill()
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.set_max_width((self.viewport.width() - 80.0).max(200.0));
                        ui.horizontal(|ui| {
                            if error {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                ui.painter().circle_filled(r.center(), 4.0, theme::DANGER);
                            }
                            ui.add(
                                egui::Label::new(RichText::new(text).size(13.5).color(theme::TEXT))
                                    .wrap(),
                            );
                        });
                    });
            });
    }

    /// The `?` sheet: every binding from the keymap, grouped, plus pointer gestures.
    fn help_sheet(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let screen = self.viewport;
        egui::Area::new(Id::new("help_backdrop"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, Sense::CLICK);
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(150));
                if resp.clicked() {
                    actions.push(Action::Help);
                }
            });
        egui::Area::new(Id::new("help_sheet"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                pill()
                    .corner_radius(16)
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        let width = (screen.width() - 64.0).clamp(320.0, 820.0);
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Shortcuts").size(18.0).color(theme::TEXT));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    keycap(ui, "Esc");
                                    ui.label(
                                        RichText::new("close").size(12.0).color(theme::TEXT_DIM),
                                    );
                                },
                            );
                        });
                        ui.add_space(14.0);
                        let columns: [&[usize]; 3] = [&[0, 1, 2], &[3, 4], &[5, 6, 7]];
                        let ncols = if width < 560.0 { 1 } else { 3 };
                        if ncols == 1 {
                            for i in 0..HELP.len() {
                                help_section(ui, i);
                            }
                        } else {
                            ui.columns(3, |cols| {
                                for (col, sections) in cols.iter_mut().zip(columns) {
                                    for &i in sections {
                                        help_section(col, i);
                                    }
                                }
                            });
                        }
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("POINTER & TYPING")
                                .size(11.0)
                                .color(theme::TEXT_FAINT),
                        );
                        ui.add_space(4.0);
                        egui::Grid::new("mouse_help")
                            .num_columns(4)
                            .spacing(Vec2::new(14.0, 6.0))
                            .show(ui, |ui| {
                                for (k, (gesture, what)) in MOUSE_HELP.iter().enumerate() {
                                    ui.label(RichText::new(*gesture).size(12.5).color(theme::TEXT));
                                    ui.label(
                                        RichText::new(*what).size(12.5).color(theme::TEXT_DIM),
                                    );
                                    if k % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
            });
    }

    fn welcome(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        egui::Area::new(Id::new("welcome"))
            .order(Order::Middle)
            .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, -20.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("nullpdf").size(44.0).color(theme::TEXT));
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("Read and mark up PDFs.")
                            .size(15.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(26.0);
                    if let Some(err) = &self.pdfium_error {
                        pill().inner_margin(Margin::same(16)).show(ui, |ui| {
                            ui.set_max_width(440.0);
                            ui.label(
                                RichText::new("libpdfium couldn't be loaded")
                                    .size(15.0)
                                    .color(theme::DANGER),
                            );
                            ui.add_space(4.0);
                            ui.label(RichText::new(err).size(12.5).color(theme::TEXT_DIM));
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(
                                    "Install pdfium (e.g. the pdfium-binaries package) \
                                         and start nullpdf again.",
                                )
                                .size(13.0)
                                .color(theme::TEXT),
                            );
                        });
                        return;
                    }
                    if open_button(ui).clicked() {
                        actions.push(Action::Open);
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let dim = |s: &str| RichText::new(s).size(13.0).color(theme::TEXT_DIM);
                        ui.label(dim("or drop one anywhere ·"));
                        keycap(ui, "?");
                        ui.label(dim("for shortcuts"));
                    });
                });
            });
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let screen = self.viewport;
        egui::Area::new(Id::new("drop_overlay"))
            .order(Order::Tooltip)
            .interactable(false)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let painter = ui.painter();
                painter.rect_filled(screen, 0.0, Color32::from_black_alpha(160));
                painter.rect_stroke(
                    screen.shrink(20.0),
                    18.0,
                    theme::line(2.0, theme::ACCENT),
                    StrokeKind::Inside,
                );
                painter.text(
                    screen.center(),
                    Align2::CENTER_CENTER,
                    "Drop to open",
                    FontId::proportional(24.0),
                    theme::TEXT,
                );
            });
    }
}

/// One cheat-sheet section: a small caps title, then keys + description per row.
fn help_section(ui: &mut Ui, index: usize) {
    let (title, rows) = HELP[index];
    ui.label(
        RichText::new(title.to_uppercase())
            .size(11.0)
            .color(theme::TEXT_FAINT),
    );
    ui.add_space(4.0);
    for row in rows {
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            let colours = row.actions.iter().all(|a| matches!(a, Action::Color(_)));
            if colours {
                keycap(ui, "1 – 7");
            } else {
                // Alternatives are separated by a slash once a chord has several caps
                // ("R / Shift R"); simple pairs like "[ ]" read fine without.
                let chords = row
                    .actions
                    .iter()
                    .any(|a| keymap::primary_caps(*a).len() > 1);
                for (k, a) in row.actions.iter().enumerate() {
                    if k > 0 {
                        if chords {
                            ui.label(RichText::new(" / ").size(12.0).color(theme::TEXT_FAINT));
                        } else {
                            ui.add_space(3.0);
                        }
                    }
                    action_caps(ui, *a);
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new(row.label).size(12.5).color(theme::TEXT_DIM));
        });
    }
    ui.add_space(12.0);
}

/// The welcome screen's call to action.
fn open_button(ui: &mut Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(212.0, 46.0), Sense::CLICK);
    let painter = ui.painter();
    let fill = if resp.hovered() {
        theme::ACCENT.gamma_multiply(1.12)
    } else {
        theme::ACCENT
    };
    painter.rect_filled(rect, 12.0, fill);
    icons::paint(
        painter,
        Rect::from_center_size(rect.left_center() + Vec2::new(26.0, 0.0), Vec2::splat(20.0)),
        Icon::Open,
        Color32::WHITE,
    );
    painter.text(
        rect.left_center() + Vec2::new(46.0, 0.0),
        Align2::LEFT_CENTER,
        "Open a PDF",
        FontId::proportional(16.0),
        Color32::WHITE,
    );
    let cap = Rect::from_center_size(
        rect.right_center() - Vec2::new(26.0, 0.0),
        Vec2::new(22.0, 22.0),
    );
    painter.rect_filled(cap, 6.0, Color32::from_white_alpha(46));
    painter.text(
        cap.center(),
        Align2::CENTER_CENTER,
        "O",
        FontId::proportional(13.0),
        Color32::WHITE,
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Middle-elide `s` to at most `max` characters, keeping the end (the extension and the
/// distinctive tail of most file names). `max == 0` leaves nothing.
fn elide(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let tail = max * 2 / 3;
    let head = max - tail - 1;
    let a: String = chars[..head].iter().collect();
    let b: String = chars[chars.len() - tail..].iter().collect();
    format!("{a}…{b}")
}
