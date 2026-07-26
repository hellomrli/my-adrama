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

    // 资产按类别分页：全部 | 角色 | 服装 | 道具 | 场景
    if stage == Stage::Assets {
        asset_tabs(ui, cx);
        ui.add_space(theme::SPACE_XS);
    }

    let mut items: Vec<ItemView> = cx.state.items(stage).to_vec();
    if stage == Stage::Assets {
        if let Some(kind) = cx.state.asset_tab {
            items.retain(|i| i.kind == crate::model::index::ItemKind::Asset(kind));
        }
    }
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

/// 资产分页签：数量画在页签上，切页不丢勾选。
fn asset_tabs(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let counts: Vec<(crate::model::AssetKind, usize, usize)> = crate::model::ASSET_KINDS
        .into_iter()
        .map(|kind| {
            let of_kind: Vec<_> = cx
                .state
                .items(Stage::Assets)
                .iter()
                .filter(|i| i.kind == crate::model::index::ItemKind::Asset(kind))
                .collect();
            let pending = of_kind
                .iter()
                .filter(|i| !matches!(i.status, ItemStatus::Done | ItemStatus::Approved))
                .count();
            (kind, of_kind.len(), pending)
        })
        .collect();
    let total: usize = counts.iter().map(|(_, n, _)| n).sum();

    let mut tab = cx.state.asset_tab;
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(tab.is_none(), format!("全部（{total}）"))
            .clicked()
        {
            tab = None;
        }
        for (kind, n, pending) in &counts {
            if *n == 0 {
                continue;
            }
            let label = if *pending > 0 {
                format!("{}（{n} · {pending} 待生成）", kind.label())
            } else {
                format!("{}（{n}）", kind.label())
            };
            if ui
                .selectable_label(tab == Some(*kind), label)
                .clicked()
            {
                tab = Some(*kind);
            }
        }
    });
    cx.state.asset_tab = tab;
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
    let checked = cx.state.checked_ids(stage);
    let has_clips = stage == Stage::Video && counts.ready > 0;

    let mut job: Option<Job> = None;
    let mut clear_checked = false;
    let mut filter = cx.state.item_filter;
    let mut thumb = cx.state.thumb_size;

    ui.horizontal_wrapped(|ui| {
        // 有勾选时，主按钮就是「只生成这些」——批量得自己另外点。
        if !checked.is_empty() {
            if widgets::primary_button(ui, &format!("生成所选（{}）", checked.len()), !busy) {
                job = Some(selection_job(stage, checked.clone()));
            }
            if widgets::button(ui, "取消勾选", true) {
                clear_checked = true;
            }
            ui.separator();
        }

        // 批量操作收进菜单：默认路径是逐条 / 按组 / 勾选，批量得自己找出来。
        ui.menu_button("批量…", |ui| {
            ui.set_min_width(180.0);
            if ui
                .add_enabled(
                    !busy && counts.pending > 0,
                    egui::Button::new(format!("生成全部缺失（{}）", counts.pending)),
                )
                .clicked()
            {
                job = Some(all_job(stage, false));
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    !busy && !failed_ids.is_empty(),
                    egui::Button::new(format!("重试全部失败（{}）", failed_ids.len())),
                )
                .clicked()
            {
                job = Some(selection_job(stage, failed_ids.clone()));
                ui.close_menu();
            }
            ui.separator();
            if ui
                .add_enabled(
                    !busy && counts.total > 0,
                    egui::Button::new(
                        RichText::new(format!("全部重生成（{}）", counts.total))
                            .color(theme::DANGER),
                    ),
                )
                .on_hover_text("会重跑每一条并覆盖已有结果（你自己上传的素材不会被动）")
                .clicked()
            {
                job = Some(all_job(stage, true));
                ui.close_menu();
            }
        });

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

    // 进度与提示单独一行，工具栏本身保持干净
    ui.horizontal(|ui| {
        widgets::meter(
            ui,
            counts.ratio(),
            theme::stage_color(stage),
            140.0,
        );
        widgets::hint(ui, &counts.summary());
        ui.separator();
        widgets::hint(
            ui,
            "勾选卡片可只生成选中项；每组标题右侧可生成该组；右侧可逐条生成或上传自己的素材",
        );
    });

    cx.state.item_filter = filter;
    if (thumb - cx.state.thumb_size).abs() > 0.5 {
        cx.state.thumb_size = thumb;
    }
    if clear_checked {
        cx.state.clear_checked(stage);
    }
    if let Some(job) = job {
        cx.state.clear_checked(stage);
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
    let mut toggle: Option<String> = None;
    let mut group_job: Option<Vec<String>> = None;

    let visible: Vec<&ItemView> = items
        .iter()
        .filter(|item| filter.accepts(cx.state.item_status(stage, item).0))
        .collect();

    // 列数自己算：horizontal_wrapped 是在超出之后才换行，会把最后一张切掉。
    let card_width = thumb + CARD_CHROME;
    let spacing = ui.spacing().item_spacing.x;
    let columns = (((ui.available_width() + spacing) / (card_width + spacing)).floor() as usize)
        .max(1);
    let busy = cx.state.is_busy();

    egui::ScrollArea::vertical()
        .id_salt(("grid", stage))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if visible.is_empty() {
                widgets::hint(ui, "当前筛选下没有条目。");
                return;
            }

            for group in group_items(stage, &visible) {
                ui.add_space(theme::SPACE_MD);
                ui.horizontal(|ui| {
                    widgets::group_title(ui, &group.title, theme::stage_color(stage));
                    let pending = group
                        .items
                        .iter()
                        .filter(|i| !matches!(i.status, ItemStatus::Done | ItemStatus::Approved))
                        .count();
                    widgets::hint(
                        ui,
                        &format!("{} 项 · {} 待生成", group.items.len(), pending),
                    );
                    if pending > 0 && widgets::button(ui, "生成本组缺失", !busy) {
                        group_job = Some(
                            group
                                .items
                                .iter()
                                .filter(|i| {
                                    !matches!(i.status, ItemStatus::Done | ItemStatus::Approved)
                                })
                                .map(|i| i.id.clone())
                                .collect(),
                        );
                    }
                });
                ui.add_space(theme::SPACE_XS);

                for row in group.items.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        for item in row {
                            let (status, detail) = cx.state.item_status(stage, item);
                            let is_selected = selected.as_deref() == Some(item.id.as_str());
                            let is_checked = cx.state.is_checked(stage, &item.id);
                            let response = card(
                                ui,
                                cx,
                                item,
                                status,
                                detail.as_deref(),
                                thumb,
                                is_selected,
                                is_checked,
                                &mut toggle,
                            );
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
            }
        });

    if let Some(id) = toggle {
        cx.state.toggle_checked(stage, &id);
    }
    if let Some(item) = clicked {
        cx.state.select(stage, &item);
    }
    if let Some(path) = preview {
        cx.state.preview = Some(path);
    }
    if let Some(ids) = group_job {
        if !ids.is_empty() {
            cx.state.submit(cx.runtime, selection_job(stage, ids));
        }
    }
}

struct Group<'a> {
    title: String,
    items: Vec<&'a ItemView>,
}

/// 资产按角色/服装/道具/场景分组，分镜与视频按场次分组——不要堆成一片。
fn group_items<'a>(stage: Stage, items: &[&'a ItemView]) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    let mut push = |title: String, item: &'a ItemView| match groups.iter_mut().find(|g| g.title == title) {
        Some(group) => group.items.push(item),
        None => groups.push(Group {
            title,
            items: vec![item],
        }),
    };

    match stage {
        Stage::Assets => {
            for kind in crate::model::ASSET_KINDS {
                for item in items {
                    if item.kind == crate::model::index::ItemKind::Asset(kind) {
                        push(kind.label().to_string(), item);
                    }
                }
            }
        }
        _ => {
            for item in items {
                let title = match item.scene {
                    Some(n) => format!("第 {n} 场"),
                    None => "未归入场次".to_string(),
                };
                push(title, item);
            }
        }
    }
    groups
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut Ui,
    cx: &mut ViewCtx<'_>,
    item: &ItemView,
    status: ItemStatus,
    detail: Option<&str>,
    thumb: f32,
    selected: bool,
    checked: bool,
    toggle: &mut Option<String>,
) -> egui::Response {
    let color = theme::item_status_color(status);
    let stroke = if selected {
        egui::Stroke::new(1.6_f32, theme::ACCENT)
    } else if checked {
        egui::Stroke::new(1.4_f32, theme::INFO)
    } else {
        egui::Stroke::new(1.0_f32, theme::BORDER)
    };

    let response = egui::Frame::new()
        .fill(if selected {
            theme::tint(theme::ACCENT, 22)
        } else if checked {
            theme::tint(theme::INFO, 18)
        } else {
            theme::SURFACE
        })
        .stroke(stroke)
        .corner_radius(theme::RADIUS_MD)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(thumb);
            ui.vertical(|ui| {
                // 勾选框：只生成挑中的几条
                ui.horizontal(|ui| {
                    let mut is_checked = checked;
                    if ui
                        .checkbox(&mut is_checked, "")
                        .on_hover_text("勾选后可在上方「生成所选」中一起生成")
                        .changed()
                    {
                        *toggle = Some(item.id.clone());
                    }
                    if item.manual {
                        widgets::pill(ui, "自备", theme::INFO);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::dot(ui, color, 8.0);
                    });
                });

                // 固定大小的图框，保证同一行卡片等高
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
                ui.add(
                    egui::Label::new(RichText::new(&item.title).strong().size(12.5)).truncate(),
                );
                let sub = detail.unwrap_or(&item.subtitle);
                ui.add(
                    egui::Label::new(RichText::new(sub).small().color(theme::TEXT_DIM)).truncate(),
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
    let mut upload = false;
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
                if item.images.len() > 1 {
                    // 分镜的多帧并排展示：首帧…末帧，一眼看出这镜怎么走
                    let width = ui.available_width();
                    let per = ((width - 8.0 * (item.images.len() - 1) as f32)
                        / item.images.len() as f32)
                        .clamp(60.0, 220.0);
                    ui.horizontal(|ui| {
                        for (i, path) in item.images.iter().enumerate() {
                            ui.vertical(|ui| {
                                let texture =
                                    cx.thumbs.get(ui.ctx(), path, (per * 2.0) as u32);
                                image_box(ui, per, per * 0.62, texture, "…");
                                // 分镜的多图是时间序列（首帧…末帧）；
                                // 资产的多图是不同视角，用文件名说话。
                                let caption = if stage == Stage::Storyboard {
                                    if i == 0 {
                                        "首帧".to_string()
                                    } else if i == item.images.len() - 1 {
                                        "末帧".to_string()
                                    } else {
                                        format!("第 {} 帧", i + 1)
                                    }
                                } else {
                                    match path.file_stem().and_then(|s| s.to_str()) {
                                        Some("front") => "正面".to_string(),
                                        Some("side") => "侧面".to_string(),
                                        Some("full") => "全身".to_string(),
                                        Some(other) => other.to_string(),
                                        None => format!("图 {}", i + 1),
                                    }
                                };
                                if ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(caption)
                                                .small()
                                                .color(theme::TEXT_DIM),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text("点击查看大图")
                                    .clicked()
                                {
                                    preview = Some(path.clone());
                                }
                            });
                        }
                    });
                } else if let Some(path) = item.thumbnail() {
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

                // 分镜帧数：全局默认，可按镜头覆盖
                if stage == Stage::Storyboard {
                    frames_control(ui, cx, &item.id);
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
                    if widgets::primary_button(ui, "生成这一条", !busy) {
                        regenerate = true;
                    }
                    if widgets::button(
                        ui,
                        if stage == Stage::Video {
                            "上传片段…"
                        } else {
                            "上传图片…"
                        },
                        !busy,
                    ) {
                        upload = true;
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

                if item.manual {
                    ui.add_space(theme::SPACE_SM);
                    widgets::hint(
                        ui,
                        "这一条是你自己上传的：批量生成不会覆盖它，只有单独点「生成这一条」才会。",
                    );
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
    if upload {
        import_file(cx, stage, &item.id);
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

/// 本镜头的分镜帧数：跟随全局或单独覆盖。改完写进 sidecar，下次生成生效。
fn frames_control(ui: &mut Ui, cx: &mut ViewCtx<'_>, shot_id: &str) {
    let Some(root) = cx.state.root() else {
        return;
    };
    let global = cx.state.config_draft.generation.frames_per_shot.clamp(2, 8);
    let current: Option<u32> = cx
        .state
        .snapshot
        .as_ref()
        .and_then(|s| {
            std::fs::read_to_string(
                s.root.join("storyboard").join(format!("{shot_id}.json")),
            )
            .ok()
        })
        .and_then(|raw| serde_json::from_str::<crate::model::StoryboardMeta>(&raw).ok())
        .and_then(|m| m.frames);

    ui.add_space(theme::SPACE_XS);
    ui.horizontal(|ui| {
        widgets::hint(ui, &format!("分镜帧数（全局 {global}）："));
        let mut value = current.unwrap_or(global);
        let changed = ui
            .add(egui::Slider::new(&mut value, 2..=8).show_value(true))
            .on_hover_text("首帧…末帧；末帧会衔接下一镜。改完点「生成这一条」补齐缺的帧")
            .drag_stopped();
        if changed && Some(value) != current && !(current.is_none() && value == global) {
            match crate::engine::set_shot_frames(&root, shot_id, Some(value)) {
                Ok(()) => {
                    cx.state.note(format!("{shot_id}：分镜帧数 = {value}"));
                    cx.state.refresh(cx.runtime);
                }
                Err(err) => cx.state.fail(format!("{err:#}")),
            }
        }
        if current.is_some() && ui.small_button("恢复全局").clicked() {
            match crate::engine::set_shot_frames(&root, shot_id, None) {
                Ok(()) => {
                    cx.state.note(format!("{shot_id}：帧数恢复跟随全局（{global}）"));
                    cx.state.refresh(cx.runtime);
                }
                Err(err) => cx.state.fail(format!("{err:#}")),
            }
        }
    });
}

/// 让用户把自己的素材放进这一条：图片统一转成 PNG，视频按 mp4 复制。
fn import_file(cx: &mut ViewCtx<'_>, stage: Stage, id: &str) {
    let Some(root) = cx.state.root() else {
        return;
    };
    let dialog = if stage == Stage::Video {
        rfd::FileDialog::new().add_filter("视频", &["mp4"])
    } else {
        rfd::FileDialog::new().add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp"])
    };
    let Some(source) = dialog.pick_file() else {
        return;
    };

    match crate::engine::import_item_file(&root, stage, id, &source) {
        Ok(target) => {
            cx.thumbs.invalidate(&target);
            cx.state.note(format!("已导入 → {}", target.display()));
            cx.state.refresh(cx.runtime);
        }
        Err(err) => cx.state.fail(format!("导入失败：{err:#}")),
    }
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
