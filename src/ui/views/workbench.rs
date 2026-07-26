//! Item review workbench, shared by 资产 / 分镜 / 视频.
//!
//! This is the screen the old UI was missing: every expected item is listed
//! with its real status, and each one can be inspected, re-prompted and
//! regenerated on its own — instead of typing an id into a text box and hoping.

use eframe::egui::{self, RichText, Ui, Vec2};

use super::ViewCtx;
use crate::engine::{stages, Job};
use crate::model::{index::truncate, ItemStatus, ItemView, Stage};
use crate::ui::state::{ItemFilter, View};
use crate::ui::{theme, widgets};

/// Card width beyond the thumbnail itself (margins + stroke).
const CARD_CHROME: f32 = 20.0;
/// Image box height as a fraction of card width.
const IMAGE_RATIO: f32 = 0.62;

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>, stage: Stage) {
    widgets::page_header(ui, View::Stage(stage).title(), View::Stage(stage).subtitle());

    if cx.state.snapshot.is_none() {
        widgets::empty_state(ui, "尚未打开项目", "在「概览」中打开或新建一个项目。");
        return;
    }

    toolbar(ui, cx, stage);
    crate::ui::views::running_banner(ui, cx);
    ui.add_space(theme::SPACE_SM);

    let items: Vec<ItemView> = cx.state.items(stage).to_vec();
    if items.is_empty() {
        widgets::empty_state(
            ui,
            "还没有可处理的条目",
            "先完成「拆解」阶段，镜头与角色表出来后这里会列出全部待生成项。",
        );
        return;
    }

    // Land on something: an empty inspector teaches the user nothing.
    if cx.state.selected_id(stage).is_none() {
        if let Some(first) = items.first() {
            let first = first.clone();
            cx.state.select(stage, &first);
        }
    }

    egui::SidePanel::right(egui::Id::new(("inspector_panel", stage)))
        .resizable(true)
        .default_width(370.0)
        .min_width(300.0)
        .max_width(560.0)
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: theme::SPACE_MD as i8,
            right: 0,
            top: 0,
            bottom: 0,
        }))
        .show_inside(ui, |ui| inspector(ui, cx, stage));

    egui::CentralPanel::default()
        .frame(egui::Frame::new())
        .show_inside(ui, |ui| grid(ui, cx, stage, &items));
}

fn toolbar(ui: &mut Ui, cx: &mut ViewCtx<'_>, stage: Stage) {
    let busy = cx.state.is_busy();
    let snapshot = cx.state.snapshot.as_ref();
    let status = snapshot.map(|s| s.state.get(stage)).unwrap_or_default();
    let counts = snapshot.map(|s| s.index.counts(stage)).unwrap_or_default();
    let failed_ids: Vec<String> = cx
        .state
        .items(stage)
        .iter()
        .filter(|i| i.status == ItemStatus::Failed)
        .map(|i| i.id.clone())
        .collect();
    let has_clips = stage == Stage::Video && counts.ready > 0;

    let mut job: Option<Job> = None;
    let mut filter = cx.state.item_filter;
    let mut thumb = cx.state.thumb_size;

    ui.horizontal_wrapped(|ui| {
        if widgets::primary_button(ui, "生成缺失", !busy) {
            job = Some(all_job(stage, false));
        }
        if widgets::button(ui, "全部重生成", !busy) {
            job = Some(all_job(stage, true));
        }
        if widgets::button(ui, &format!("重试失败（{}）", failed_ids.len()), !busy && !failed_ids.is_empty())
        {
            job = Some(selection_job(stage, failed_ids.clone()));
        }
        if has_clips && widgets::button(ui, "拼接成片", !busy) {
            job = Some(Job::Export);
        }

        ui.separator();
        if status.is_approved() {
            if widgets::button(ui, "撤销审核", !busy) {
                job = Some(Job::Reset(stage));
            }
        } else if widgets::button(ui, "审核通过", !busy) {
            job = Some(Job::Approve(stage));
        }
        widgets::pill(ui, status.label(), theme::stage_status_color(status));
        widgets::hint(ui, &counts.summary());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Slider::new(&mut thumb, 110.0..=280.0)
                    .show_value(false)
                    .trailing_fill(true),
            )
            .on_hover_text("缩略图大小");
            // A right-to-left layout emits widgets in reverse order.
            for option in ItemFilter::ALL.into_iter().rev() {
                if ui
                    .selectable_label(filter == option, option.label())
                    .clicked()
                {
                    filter = option;
                }
            }
        });
    });

    cx.state.item_filter = filter;
    if (thumb - cx.state.thumb_size).abs() > 0.5 {
        cx.state.thumb_size = thumb;
    }
    if let Some(job) = job {
        cx.state.submit(cx.runtime, job);
    }
}

fn all_job(stage: Stage, force: bool) -> Job {
    match stage {
        Stage::Assets => Job::Assets(stages::assets::Selection {
            force,
            ..Default::default()
        }),
        Stage::Storyboard => Job::Storyboard(stages::storyboard::Selection {
            force,
            ..Default::default()
        }),
        Stage::Video => Job::Video(stages::video::Selection {
            force,
            ..Default::default()
        }),
        Stage::Parse => Job::Parse,
    }
}

fn selection_job(stage: Stage, ids: Vec<String>) -> Job {
    match stage {
        Stage::Assets => Job::Assets(stages::assets::Selection::only(ids)),
        Stage::Storyboard => Job::Storyboard(stages::storyboard::Selection::only(ids)),
        Stage::Video => Job::Video(stages::video::Selection::only(ids)),
        Stage::Parse => Job::Parse,
    }
}

fn grid(ui: &mut Ui, cx: &mut ViewCtx<'_>, stage: Stage, items: &[ItemView]) {
    let filter = cx.state.item_filter;
    let thumb = cx.state.thumb_size;
    let selected = cx.state.selected_id(stage).map(str::to_string);
    let mut clicked: Option<ItemView> = None;
    let mut preview: Option<std::path::PathBuf> = None;

    let visible: Vec<&ItemView> = items
        .iter()
        .filter(|item| filter.accepts(cx.state.item_status(stage, item).0))
        .collect();

    // Columns are computed rather than left to `horizontal_wrapped`, which
    // decides after an item has already overflowed the row.
    let card_width = thumb + CARD_CHROME;
    let spacing = ui.spacing().item_spacing.x;
    let columns = (((ui.available_width() + spacing) / (card_width + spacing)).floor() as usize)
        .max(1);

    egui::ScrollArea::vertical()
        .id_salt(("grid", stage))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if visible.is_empty() {
                widgets::hint(ui, "当前筛选下没有条目。");
                return;
            }
            for row in visible.chunks(columns) {
                ui.horizontal_top(|ui| {
                    for item in row {
                        let (status, detail) = cx.state.item_status(stage, item);
                        let is_selected = selected.as_deref() == Some(item.id.as_str());
                        let response =
                            card(ui, cx, item, status, detail.as_deref(), thumb, is_selected);
                        if response.clicked() {
                            clicked = Some((*item).clone());
                        }
                        if response.double_clicked() {
                            if let Some(path) = item.thumbnail() {
                                preview = Some(path.to_path_buf());
                            }
                        }
                    }
                });
            }
        });

    if let Some(item) = clicked {
        cx.state.select(stage, &item);
    }
    if let Some(path) = preview {
        cx.state.preview = Some(path);
    }
}

fn card(
    ui: &mut Ui,
    cx: &mut ViewCtx<'_>,
    item: &ItemView,
    status: ItemStatus,
    detail: Option<&str>,
    thumb: f32,
    selected: bool,
) -> egui::Response {
    let color = theme::item_status_color(status);
    let stroke = if selected {
        egui::Stroke::new(1.6_f32, theme::ACCENT)
    } else {
        egui::Stroke::new(1.0_f32, theme::BORDER)
    };

    let response = egui::Frame::new()
        .fill(if selected {
            theme::tint(theme::ACCENT, 22)
        } else {
            theme::SURFACE
        })
        .stroke(stroke)
        .corner_radius(theme::RADIUS_MD)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(thumb);
            ui.vertical(|ui| {
            // Fixed-size image box keeps every card the same height whatever
            // the frame aspect ratio is.
            let texture = item
                .thumbnail()
                .and_then(|path| cx.thumbs.get(ui.ctx(), path, (thumb * 2.0) as u32));
            let placeholder = if item.thumbnail().is_some() {
                "加载中…"
            } else {
                "未生成"
            };
            image_box(ui, thumb, thumb * IMAGE_RATIO, texture, placeholder);

            ui.add_space(theme::SPACE_XS);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(RichText::new(&item.title).strong().size(12.5)).truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    widgets::dot(ui, color, 8.0);
                });
            });
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(item.kind.label())
                        .small()
                        .color(theme::TEXT_DIM),
                );
                if let Some(scene) = item.scene {
                    ui.label(
                        RichText::new(format!("第 {scene} 场"))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                }
            });
            let sub = detail.unwrap_or(&item.subtitle);
            // Truncate rather than wrap so every card in a row is the same height.
            ui.add(
                egui::Label::new(RichText::new(sub).small().color(theme::TEXT_DIM))
                    .truncate(),
            );
            });
        })
        .response
        .interact(egui::Sense::click());

    response.on_hover_text(format!("{} · {}", item.id, status.label()))
}

/// Draw an image letterboxed into a fixed box, or a placeholder if absent.
fn image_box(
    ui: &mut Ui,
    width: f32,
    height: f32,
    texture: Option<egui::TextureHandle>,
    placeholder: &str,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, theme::RADIUS_SM, theme::INPUT);

    match texture {
        Some(texture) => {
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let target = egui::Rect::from_center_size(rect.center(), size * scale);
            painter.image(
                texture.id(),
                target,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                placeholder,
                egui::TextStyle::Small.resolve(ui.style()),
                theme::TEXT_DIM,
            );
        }
    }
}

fn inspector(ui: &mut Ui, cx: &mut ViewCtx<'_>, stage: Stage) {
    let Some(item) = cx.state.selected_item(stage).cloned() else {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::section_title(ui, "条目详情");
            ui.add_space(theme::SPACE_SM);
            widgets::hint(ui, "点击左侧任意条目查看详情、修改 prompt 并单独重生成。");
        });
        return;
    };

    let busy = cx.state.is_busy();
    let (status, detail) = cx.state.item_status(stage, &item);
    let mut regenerate = false;
    let mut save_prompt = false;
    let mut reset_prompt = false;
    let mut preview: Option<std::path::PathBuf> = None;

    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical()
            .id_salt(("inspector", stage))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::section_title(ui, &item.title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::pill(ui, status.label(), theme::item_status_color(status));
                    });
                });
                ui.label(RichText::new(&item.id).small().monospace().color(theme::TEXT_DIM));
                if let Some(detail) = &detail {
                    ui.label(RichText::new(detail).small().color(theme::INFO));
                }
                ui.label(RichText::new(&item.subtitle).small().color(theme::TEXT_MUTED));

                ui.add_space(theme::SPACE_SM);
                if let Some(path) = item.thumbnail() {
                    // Cap the preview so the prompt editor and actions stay
                    // above the fold — they are what this panel is for.
                    let width = ui.available_width();
                    let height = (ui.available_height() * 0.40).clamp(150.0, 300.0);
                    let texture = cx.thumbs.get(ui.ctx(), path, (width * 2.0) as u32);
                    image_box(ui, width, height, texture, "加载中…");
                    let response = ui.interact(
                        egui::Rect::from_min_size(
                            ui.min_rect().left_bottom() - egui::vec2(0.0, height),
                            Vec2::new(width, height),
                        ),
                        ui.id().with(("preview", &item.id)),
                        egui::Sense::click(),
                    );
                    if response.on_hover_text("点击查看大图").clicked() {
                        preview = Some(path.to_path_buf());
                    }
                }

                if let Some(error) = &item.error {
                    ui.add_space(theme::SPACE_SM);
                    theme::inset().show(ui, |ui| {
                        ui.label(RichText::new("失败原因").small().color(theme::DANGER));
                        ui.label(
                            RichText::new(truncate(error, 400))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                }

                ui.add_space(theme::SPACE_MD);
                prompt_editor(ui, cx, stage, &item, &mut save_prompt, &mut reset_prompt);

                ui.add_space(theme::SPACE_MD);
                ui.horizontal_wrapped(|ui| {
                    if widgets::primary_button(ui, "重新生成此条", !busy) {
                        regenerate = true;
                    }
                    if let Some(media) = &item.media {
                        if widgets::button(ui, "播放片段", true) {
                            widgets::open_path(media);
                        }
                    }
                    if let Some(path) = item.thumbnail() {
                        if widgets::button(ui, "打开目录", true) {
                            if let Some(parent) = path.parent() {
                                widgets::open_path(parent);
                            }
                        }
                    }
                });

                if !item.references.is_empty() {
                    ui.add_space(theme::SPACE_MD);
                    widgets::section_title(
                        ui,
                        if stage == Stage::Video {
                            "任务 id"
                        } else {
                            "参考图"
                        },
                    );
                    for reference in &item.references {
                        ui.label(
                            RichText::new(widgets::shorten_path(reference, 46))
                                .small()
                                .monospace()
                                .color(theme::TEXT_DIM),
                        )
                        .on_hover_text(reference);
                    }
                }

                if let Some(seconds) = item.duration_secs {
                    ui.add_space(theme::SPACE_SM);
                    widgets::hint(ui, &format!("时长 {seconds} 秒"));
                }
            });
    });

    if let Some(path) = preview {
        cx.state.preview = Some(path);
    }
    if save_prompt || reset_prompt {
        persist_prompt(cx, stage, &item.id, reset_prompt);
    }
    if regenerate {
        cx.state
            .submit(cx.runtime, selection_job(stage, vec![item.id.clone()]));
    }
}

fn prompt_editor(
    ui: &mut Ui,
    cx: &mut ViewCtx<'_>,
    stage: Stage,
    item: &ItemView,
    save: &mut bool,
    reset: &mut bool,
) {
    // Make sure the editor is bound to the selected item.
    let bound = cx
        .state
        .prompt_edit
        .as_ref()
        .map(|e| e.stage == stage && e.id == item.id)
        .unwrap_or(false);
    if !bound {
        cx.state.prompt_edit = Some(crate::ui::state::PromptEdit {
            stage,
            id: item.id.clone(),
            text: item.prompt.clone(),
            dirty: false,
        });
    }

    let Some(edit) = cx.state.prompt_edit.as_mut() else {
        return;
    };
    ui.horizontal(|ui| {
        widgets::section_title(ui, "Prompt");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if edit.dirty {
                widgets::pill(ui, "未保存", theme::WARNING);
            }
        });
    });
    widgets::hint(ui, "保存后，下次生成这一条会使用你修改的文本。");
    let response = ui.add(
        egui::TextEdit::multiline(&mut edit.text)
            .desired_width(f32::INFINITY)
            .desired_rows(6)
            .font(egui::TextStyle::Monospace),
    );
    if response.changed() {
        edit.dirty = true;
    }
    let dirty = edit.dirty;

    ui.horizontal(|ui| {
        if widgets::button(ui, "保存 Prompt", dirty) {
            *save = true;
        }
        if widgets::button(ui, "恢复默认", true) {
            *reset = true;
        }
    });
}

/// Write the edited prompt back to the sidecar (or clear it, restoring the
/// composed default).
fn persist_prompt(cx: &mut ViewCtx<'_>, stage: Stage, id: &str, reset: bool) {
    let Some(root) = cx.state.root() else {
        return;
    };
    let text = if reset {
        String::new()
    } else {
        cx.state
            .prompt_edit
            .as_ref()
            .map(|e| e.text.clone())
            .unwrap_or_default()
    };

    match crate::engine::save_prompt(&root, stage, id, &text) {
        Ok(()) => {
            if let Some(edit) = cx.state.prompt_edit.as_mut() {
                edit.dirty = false;
                if reset {
                    edit.text.clear();
                }
            }
            cx.state.note(if reset {
                format!("{id}：已恢复默认 prompt")
            } else {
                format!("{id}：prompt 已保存")
            });
            cx.state.refresh(cx.runtime);
        }
        Err(err) => cx.state.fail(format!("保存 prompt 失败：{err:#}")),
    }
}
