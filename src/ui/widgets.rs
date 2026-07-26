//! Small reusable pieces. Views compose these instead of re-deriving the same
//! frame/colour/spacing combinations by hand.

use eframe::egui::{self, Color32, Response, RichText, Sense, Stroke, Ui, Vec2};
use std::path::Path;

use super::theme;

/// Rounded status chip.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    egui::Frame::new()
        .fill(theme::tint(color, 38))
        .stroke(Stroke::new(1.0_f32, theme::tint(color, 110)))
        .corner_radius(999.0_f32)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        })
        .response
}

/// Small filled circle, e.g. a stage colour marker.
pub fn dot(ui: &mut Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
}

/// Page title + one-line explanation.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(21.0).strong().color(theme::TEXT));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(theme::TEXT_MUTED));
    }
    ui.add_space(theme::SPACE_MD);
}

/// Heading inside a card.
pub fn section_title(ui: &mut Ui, title: &str) {
    ui.horizontal(|ui| {
        theme::accent_bar(ui, theme::tint(theme::ACCENT, 160), 15.0);
        ui.add_space(2.0);
        ui.label(RichText::new(title).size(14.5).strong().color(theme::TEXT));
    });
}

/// 带颜色的分组标题（资产类别、场次）。
pub fn group_title(ui: &mut Ui, title: &str, color: Color32) {
    ui.horizontal(|ui| {
        theme::accent_bar(ui, color, 16.0);
        ui.add_space(2.0);
        ui.label(RichText::new(title).size(14.0).strong().color(theme::TEXT));
    });
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::TEXT_DIM));
}

pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(theme::TEXT).strong())
            .fill(theme::ACCENT_DIM)
            .stroke(Stroke::new(1.0_f32, theme::ACCENT)),
    )
    .clicked()
}

pub fn button(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(text)).clicked()
}

pub fn danger_button(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(theme::DANGER))
            .fill(theme::tint(theme::DANGER, 30))
            .stroke(Stroke::new(1.0_f32, theme::tint(theme::DANGER, 120))),
    )
    .clicked()
}

/// Compact metric: big value over a small label.
pub fn stat(ui: &mut Ui, value: &str, label: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(value).size(19.0).strong().color(color));
        ui.label(RichText::new(label).small().color(theme::TEXT_DIM));
    });
}

/// Centred placeholder for an empty list.
pub fn empty_state(ui: &mut Ui, title: &str, hint_text: &str) {
    theme::inset().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE_LG);
            ui.label(RichText::new(title).color(theme::TEXT_MUTED));
            if !hint_text.is_empty() {
                ui.label(RichText::new(hint_text).small().color(theme::TEXT_DIM));
            }
            ui.add_space(theme::SPACE_LG);
        });
    });
}

/// Label + widget on one row with a fixed, right-aligned label column.
/// 右对齐让不同长度的标签与控件之间保持同一条竖线，比左对齐整齐得多。
pub fn field_row(ui: &mut Ui, label: &str, label_width: f32, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(label_width, 22.0), Sense::hover());
        ui.painter().text(
            rect.right_center() - Vec2::new(10.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::TextStyle::Body.resolve(ui.style()),
            theme::TEXT_MUTED,
        );
        add(ui);
    });
}

/// Single-line text field bound to a string, reporting whether it changed.
pub fn text_field(ui: &mut Ui, value: &mut String, width: f32, hint_text: &str) -> bool {
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .hint_text(hint_text),
    )
    .changed()
}

/// Secret field with a reveal toggle, so users can verify a pasted key without
/// exposing it by default.
pub fn secret_field(
    ui: &mut Ui,
    value: &mut String,
    revealed: &mut bool,
    width: f32,
    hint_text: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = ui
            .add(
                egui::TextEdit::singleline(value)
                    .password(!*revealed)
                    .desired_width(width)
                    .hint_text(hint_text),
            )
            .changed();
        let icon = if *revealed { "隐藏" } else { "显示" };
        if ui
            .add_enabled(!value.is_empty(), egui::Button::new(RichText::new(icon).small()))
            .clicked()
        {
            *revealed = !*revealed;
        }
    });
    changed
}

/// 「正在跑」的状态条：放在阶段页面顶部，用户点完按钮就在原地看得到反馈。
/// 返回 true 表示点了取消。
pub fn running_strip(
    ui: &mut Ui,
    label: &str,
    detail: Option<&str>,
    elapsed: std::time::Duration,
    progress: Option<(u32, u32)>,
) -> bool {
    let mut cancel = false;
    egui::Frame::new()
        .fill(theme::tint(theme::INFO, 26))
        .stroke(Stroke::new(1.0_f32, theme::tint(theme::INFO, 110)))
        .corner_radius(theme::RADIUS_MD)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(label).strong().color(theme::TEXT));
                ui.label(
                    RichText::new(format!("{} 秒", elapsed.as_secs()))
                        .small()
                        .monospace()
                        .color(theme::TEXT_DIM),
                );
                if let Some((done, total)) = progress {
                    if total > 0 {
                        ui.add(
                            egui::ProgressBar::new(done as f32 / total as f32)
                                .desired_width(160.0)
                                .text(format!("{done}/{total}")),
                        );
                    }
                }
                if let Some(detail) = detail {
                    ui.label(RichText::new(detail).color(theme::INFO));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if danger_button(ui, "取消", true) {
                        cancel = true;
                    }
                });
            });
        });
    cancel
}

/// Thin horizontal meter used for stage completion.
pub fn meter(ui: &mut Ui, ratio: f32, color: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, theme::INPUT);
    if ratio > 0.0 {
        let mut filled = rect;
        filled.set_width(rect.width() * ratio.clamp(0.0, 1.0));
        painter.rect_filled(filled, 3.0, color);
    }
}

/// A clickable row that looks like a list item.
pub fn selectable_row(ui: &mut Ui, selected: bool, add: impl FnOnce(&mut Ui)) -> Response {
    let fill = if selected {
        theme::tint(theme::ACCENT, 22)
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        Stroke::new(1.0_f32, theme::tint(theme::ACCENT, 140))
    } else {
        Stroke::new(1.0_f32, theme::tint(theme::BORDER, 140))
    };
    let response = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(theme::RADIUS_MD)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response
        .interact(Sense::click());

    if response.hovered() && !selected {
        ui.painter().rect_filled(
            response.rect,
            theme::RADIUS_MD,
            Color32::from_rgba_unmultiplied(255, 255, 255, 8),
        );
    }
    response
}

/// Copyable monospace path, shortened in the middle when long.
pub fn path_label(ui: &mut Ui, path: &Path) {
    let text = shorten_path(&path.display().to_string(), 64);
    ui.label(RichText::new(text).small().monospace().color(theme::TEXT_DIM))
        .on_hover_text(path.display().to_string());
}

pub fn shorten_path(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head: String = chars.iter().take(max / 3).collect();
    let tail: String = chars.iter().skip(chars.len() - max * 2 / 3).collect();
    format!("{head}…{tail}")
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) {
    open_path(Path::new(url));
}

/// Open a file, folder or URL with the desktop's default handler.
pub fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.display().to_string()])
        .spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();

    if let Err(err) = result {
        tracing::warn!("打开 {} 失败：{err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_shorten_in_the_middle() {
        let long = "/home/user/projects/".repeat(6);
        let short = shorten_path(&long, 30);
        assert!(short.chars().count() <= 31);
        assert!(short.contains('…'));
        assert_eq!(shorten_path("/tmp/a", 30), "/tmp/a");
    }
}
