//! Visual theme tokens for the redesigned UI.

use eframe::egui::{self, Color32, Shadow, Stroke, Style, Visuals};

pub const BG: Color32 = Color32::from_rgb(14, 15, 20);
pub const BG_PANEL: Color32 = Color32::from_rgb(20, 22, 30);
pub const BG_ELEVATED: Color32 = Color32::from_rgb(28, 31, 42);
pub const BG_HOVER: Color32 = Color32::from_rgb(36, 40, 54);
pub const BG_INPUT: Color32 = Color32::from_rgb(18, 20, 28);

pub const BORDER: Color32 = Color32::from_rgb(48, 52, 68);

pub const TEXT: Color32 = Color32::from_rgb(232, 234, 242);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(140, 148, 168);
pub const TEXT_DIM: Color32 = Color32::from_rgb(96, 104, 124);

pub const ACCENT: Color32 = Color32::from_rgb(88, 140, 255);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(48, 72, 140);
pub const SUCCESS: Color32 = Color32::from_rgb(72, 190, 130);
pub const WARNING: Color32 = Color32::from_rgb(230, 180, 70);
pub const DANGER: Color32 = Color32::from_rgb(230, 90, 100);
pub const INFO: Color32 = Color32::from_rgb(100, 190, 230);

pub const STAGE_SCRIPT: Color32 = Color32::from_rgb(72, 130, 200);
pub const STAGE_PARSE: Color32 = Color32::from_rgb(80, 170, 120);
pub const STAGE_ASSETS: Color32 = Color32::from_rgb(210, 150, 70);
pub const STAGE_STORY: Color32 = Color32::from_rgb(160, 110, 220);
pub const STAGE_VIDEO: Color32 = Color32::from_rgb(220, 90, 120);
pub const STAGE_EXPORT: Color32 = Color32::from_rgb(100, 130, 160);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.indent = 18.0;
    style.interaction.selectable_labels = false;

    let mut v = Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;
    v.faint_bg_color = BG_ELEVATED;
    v.extreme_bg_color = BG_INPUT;
    v.code_bg_color = BG_INPUT;
    v.window_fill = BG_PANEL;
    v.panel_fill = BG;
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(80),
    };
    v.window_shadow = v.popup_shadow;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);

    v.widgets.noninteractive.bg_fill = BG_ELEVATED;
    v.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_MUTED);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);

    v.widgets.inactive.bg_fill = BG_ELEVATED;
    v.widgets.inactive.weak_bg_fill = BG_ELEVATED;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);

    v.widgets.hovered.bg_fill = BG_HOVER;
    v.widgets.hovered.weak_bg_fill = BG_HOVER;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_SOFT);

    v.widgets.active.bg_fill = ACCENT_SOFT;
    v.widgets.active.weak_bg_fill = ACCENT_SOFT;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.active.bg_stroke = Stroke::new(1.5_f32, ACCENT);

    v.widgets.open.bg_fill = BG_HOVER;
    v.widgets.open.weak_bg_fill = BG_HOVER;
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);

    v.selection.bg_fill = Color32::from_rgba_unmultiplied(88, 140, 255, 60);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    style.visuals = v;
    // silence unused import warning for Style if only used above
    let _: &Style = &style;
    ctx.set_style(style);
}

pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::same(16))
}

pub fn section_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_ELEVATED)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(14))
}

pub fn rail_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 12,
            bottom: 12,
        })
}
