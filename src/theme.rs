//! Visual design tokens (colours, metrics) and the egui style built from them.
//!
//! The chrome is a quiet dark grey so white pages carry the contrast; one blue accent marks
//! focus/selection. Everything floats over the canvas in rounded "pills".

use egui::{Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle};

/// Canvas behind the pages.
pub const CANVAS: Color32 = Color32::from_rgb(28, 29, 32);
/// Floating chrome surfaces (toolbar, status, toasts, sheets).
pub const SURFACE: Color32 = Color32::from_rgb(38, 39, 44);
/// Hover/pressed state on a surface.
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(52, 53, 59);
/// Active tool background.
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(68, 70, 78);
/// 1px outline around surfaces.
pub const BORDER: Color32 = Color32::from_rgb(58, 59, 66);
/// Primary text/icon colour.
pub const TEXT: Color32 = Color32::from_rgb(232, 232, 236);
/// Secondary text (labels, status).
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 151, 160);
/// Disabled icons.
pub const TEXT_FAINT: Color32 = Color32::from_rgb(92, 93, 102);
/// Focus / selection accent (also the text caret, which must read on white paper).
pub const ACCENT: Color32 = Color32::from_rgb(64, 132, 255);
/// Errors.
pub const DANGER: Color32 = Color32::from_rgb(255, 99, 92);
/// Page placeholder while its texture renders (paper, not a dark hole).
pub const PAPER: Color32 = Color32::from_rgb(246, 246, 244);

/// Corner radius of floating surfaces.
pub const PILL_RADIUS: u8 = 12;
/// Height of a toolbar button (square).
pub const BUTTON: f32 = 32.0;
/// Gap between the window edge and floating chrome.
pub const MARGIN: f32 = 12.0;

/// A stroke of `width` screen px. (Typed, so float literals don't hit inference fallback.)
pub fn line(width: f32, color: Color32) -> Stroke {
    Stroke::new(width, color)
}

pub fn shadow() -> Shadow {
    Shadow {
        offset: [0, 4],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(110),
    }
}

/// Install the app's visuals on `ctx` (dark only: documents read best on a dark surround).
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    // Clicking the chrome (a swatch, the size slider) while typing must restyle the text
    // being edited, not end the edit. Presses on the canvas commit it explicitly instead.
    ctx.options_mut(|o| o.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never);
    ctx.global_style_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = CANVAS;
        v.window_fill = SURFACE;
        v.extreme_bg_color = Color32::from_rgb(24, 25, 28);
        v.faint_bg_color = SURFACE_HOVER;
        v.window_stroke = line(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(PILL_RADIUS);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_shadow = shadow();
        v.popup_shadow = shadow();
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
        v.selection.stroke = line(1.0, ACCENT);
        v.text_cursor.stroke = line(2.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.interact_cursor = Some(egui::CursorIcon::PointingHand);

        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.noninteractive, SURFACE),
            (&mut w.inactive, SURFACE_HOVER),
            (&mut w.hovered, SURFACE_ACTIVE),
            (&mut w.active, SURFACE_ACTIVE),
            (&mut w.open, SURFACE_ACTIVE),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = CornerRadius::same(8);
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke.color = TEXT;
        }
        w.noninteractive.bg_stroke = line(1.0, BORDER);
        w.noninteractive.fg_stroke.color = TEXT_DIM;

        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.interaction.tooltip_delay = 0.35;
        style.interaction.show_tooltips_only_when_still = false;
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
    });
}
