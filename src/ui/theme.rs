//! Design tokens and egui style.
//!
//! Colours, spacing and type sizes live here only — views never hard-code a
//! hex value or a magic margin, which is what let the old UI drift into a
//! dozen slightly different greys.

use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Shadow, Stroke, TextStyle, Visuals};

use crate::model::{ItemStatus, Stage, StageStatus};

// --- surfaces ---------------------------------------------------------------
pub const BG: Color32 = Color32::from_rgb(13, 14, 18);
pub const SURFACE: Color32 = Color32::from_rgb(20, 22, 28);
pub const SURFACE_ALT: Color32 = Color32::from_rgb(26, 29, 37);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(34, 38, 48);
pub const INPUT: Color32 = Color32::from_rgb(16, 18, 24);

pub const BORDER: Color32 = Color32::from_rgb(42, 46, 58);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(62, 68, 84);

// --- text -------------------------------------------------------------------
pub const TEXT: Color32 = Color32::from_rgb(232, 235, 243);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(150, 158, 176);
pub const TEXT_DIM: Color32 = Color32::from_rgb(104, 112, 132);

// --- semantic ---------------------------------------------------------------
pub const ACCENT: Color32 = Color32::from_rgb(108, 152, 255);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(46, 66, 122);
pub const SUCCESS: Color32 = Color32::from_rgb(86, 196, 140);
pub const WARNING: Color32 = Color32::from_rgb(226, 176, 78);
pub const DANGER: Color32 = Color32::from_rgb(232, 100, 108);
pub const INFO: Color32 = Color32::from_rgb(108, 190, 226);

// --- spacing scale ----------------------------------------------------------
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 8.0;
pub const SPACE_MD: f32 = 12.0;
pub const SPACE_LG: f32 = 18.0;

pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_MD: f32 = 10.0;
pub const RADIUS_LG: f32 = 14.0;

/// Colour that identifies a stage across nav, cards and the pipeline strip.
pub fn stage_color(stage: Stage) -> Color32 {
    match stage {
        Stage::Parse => Color32::from_rgb(96, 178, 132),
        Stage::Assets => Color32::from_rgb(214, 158, 84),
        Stage::Storyboard => Color32::from_rgb(166, 124, 224),
        Stage::Video => Color32::from_rgb(224, 106, 132),
    }
}

pub fn stage_status_color(status: StageStatus) -> Color32 {
    match status {
        StageStatus::Pending => TEXT_DIM,
        StageStatus::InProgress => INFO,
        StageStatus::Done => SUCCESS,
        StageStatus::Approved => WARNING,
    }
}

pub fn item_status_color(status: ItemStatus) -> Color32 {
    match status {
        ItemStatus::Pending => TEXT_DIM,
        ItemStatus::Generating => INFO,
        ItemStatus::Done => SUCCESS,
        ItemStatus::Failed => DANGER,
        ItemStatus::Approved => WARNING,
    }
}

/// Translucent tint of a colour, for chip backgrounds and selected rows.
pub fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_SM);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.indent = 18.0;
    style.spacing.scroll.bar_width = 10.0;
    style.interaction.selectable_labels = false;
    style.visuals = visuals();

    // A type scale, rather than egui's defaults which are tuned for latin text.
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Body, FontId::proportional(13.5)),
        (TextStyle::Button, FontId::proportional(13.5)),
        (TextStyle::Heading, FontId::proportional(19.0)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();

    ctx.set_style(style);
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;
    v.faint_bg_color = SURFACE_ALT;
    v.extreme_bg_color = INPUT;
    v.code_bg_color = INPUT;
    v.window_fill = SURFACE;
    v.panel_fill = BG;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(RADIUS_LG as u8);
    v.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(96),
    };
    v.window_shadow = v.popup_shadow;

    v.widgets.noninteractive.bg_fill = SURFACE_ALT;
    v.widgets.noninteractive.weak_bg_fill = SURFACE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_MUTED);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS_SM as u8);

    v.widgets.inactive.bg_fill = SURFACE_ALT;
    v.widgets.inactive.weak_bg_fill = SURFACE_ALT;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.corner_radius = CornerRadius::same(RADIUS_SM as u8);

    v.widgets.hovered.bg_fill = SURFACE_HOVER;
    v.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER_STRONG);
    v.widgets.hovered.corner_radius = CornerRadius::same(RADIUS_SM as u8);

    v.widgets.active.bg_fill = ACCENT_DIM;
    v.widgets.active.weak_bg_fill = ACCENT_DIM;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.corner_radius = CornerRadius::same(RADIUS_SM as u8);

    v.widgets.open.bg_fill = SURFACE_HOVER;
    v.widgets.open.weak_bg_fill = SURFACE_HOVER;
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);

    v.selection.bg_fill = tint(ACCENT, 64);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v
}

// --- frames -----------------------------------------------------------------

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(RADIUS_LG)
        .inner_margin(Margin::same(16))
}

pub fn inset() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE_ALT)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(RADIUS_MD)
        .inner_margin(Margin::same(12))
}

pub fn bar() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(Margin::symmetric(16, 10))
}

pub fn rail() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 14,
            bottom: 12,
        })
}

pub fn content() -> egui::Frame {
    egui::Frame::new()
        .fill(BG)
        .inner_margin(Margin::symmetric(20, 16))
}
