//! Project overview: where you are in the pipeline and what to do next.

use eframe::egui::{self, RichText, Ui};

use super::ViewCtx;
use crate::engine::Job;
use crate::model::{AspectRatio, Capability, Project, Stage, StageStatus};
use crate::ui::state::View;
use crate::ui::{theme, widgets};

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    if cx.state.snapshot.is_none() {
        welcome(ui, cx);
        return;
    }

    widgets::page_header(ui, View::Dashboard.title(), View::Dashboard.subtitle());
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            summary_card(ui, cx);
            ui.add_space(theme::SPACE_MD);
            pipeline(ui, cx);
            ui.add_space(theme::SPACE_MD);
            readiness(ui, cx);
        });
}

fn summary_card(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let Some(snapshot) = &cx.state.snapshot else {
        return;
    };
    let shots = snapshot.breakdown.as_ref().map(|b| b.shots.len()).unwrap_or(0);
    let seconds = snapshot
        .breakdown
        .as_ref()
        .map(|b| b.total_seconds())
        .unwrap_or(0);
    let assets = snapshot.index.counts(Stage::Assets);
    let frames = snapshot.index.counts(Stage::Storyboard);
    let clips = snapshot.index.counts(Stage::Video);
    let style = snapshot.config.style.clone();
    let aspect = snapshot.config.aspect;
    let final_cut = snapshot.index.final_cut.clone();

    // 流水线一览：每段的完成度与状态，画成一条相连的带子
    let strip: Vec<(Stage, StageStatus, f32)> = Stage::ALL
        .into_iter()
        .map(|stage| {
            let status = snapshot.state.get(stage);
            let ratio = match stage {
                Stage::Parse => {
                    if snapshot.breakdown.is_some() {
                        1.0
                    } else {
                        0.0
                    }
                }
                other => snapshot.index.counts(other).ratio(),
            };
            (stage, status, ratio)
        })
        .collect();

    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::stat(ui, &shots.to_string(), "镜头", theme::TEXT);
            ui.add_space(theme::SPACE_LG);
            widgets::stat(
                ui,
                &format!("{}/{}", assets.ready, assets.total),
                "资产",
                theme::stage_color(Stage::Assets),
            );
            ui.add_space(theme::SPACE_LG);
            widgets::stat(
                ui,
                &format!("{}/{}", frames.ready, frames.total),
                "分镜",
                theme::stage_color(Stage::Storyboard),
            );
            ui.add_space(theme::SPACE_LG);
            widgets::stat(
                ui,
                &format!("{}/{}", clips.ready, clips.total),
                "片段",
                theme::stage_color(Stage::Video),
            );
            ui.add_space(theme::SPACE_LG);
            widgets::stat(ui, &format!("{seconds}s"), "预计时长", theme::TEXT_MUTED);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(path) = &final_cut {
                    if widgets::primary_button(ui, "播放成片", true) {
                        widgets::open_path(path);
                    }
                }
            });
        });
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            widgets::pill(ui, aspect.as_str(), theme::ACCENT);
            ui.label(RichText::new(style).small().color(theme::TEXT_MUTED));
        });

        // 流水线带：四段相连，颜色即阶段，填充即完成度，★ 表示已审核
        ui.add_space(theme::SPACE_MD);
        let gap = 6.0;
        let seg_w = ((ui.available_width() - 3.0 * gap) / 4.0).max(60.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (stage, status, ratio) in &strip {
                ui.allocate_ui(egui::vec2(seg_w, 30.0), |ui| {
                    ui.vertical(|ui| {
                        widgets::meter(ui, *ratio, theme::stage_color(*stage), seg_w);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{} {}", stage.ordinal(), stage.label()))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(status.glyph())
                                            .small()
                                            .color(theme::stage_status_color(*status)),
                                    );
                                },
                            );
                        });
                    });
                });
            }
        });
    });
}

fn pipeline(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let Some(snapshot) = &cx.state.snapshot else {
        return;
    };
    let busy = cx.state.is_busy();
    let states: Vec<(Stage, StageStatus, String, f32, bool)> = Stage::ALL
        .into_iter()
        .map(|stage| {
            let status = snapshot.state.get(stage);
            let (summary, ratio) = match stage {
                Stage::Parse => (
                    snapshot
                        .breakdown
                        .as_ref()
                        .map(|b| format!("{} 镜头 · {} 角色", b.shots.len(), b.characters.len()))
                        .unwrap_or_else(|| "尚未拆解".into()),
                    if snapshot.breakdown.is_some() { 1.0 } else { 0.0 },
                ),
                other => {
                    let counts = snapshot.index.counts(other);
                    (counts.summary(), counts.ratio())
                }
            };
            let unlocked = stage
                .prev()
                .map(|p| snapshot.state.get(p).is_approved())
                .unwrap_or(true);
            (stage, status, summary, ratio, unlocked)
        })
        .collect();

    let mut action: Option<(Stage, StageAction)> = None;

    ui.columns(4, |cols| {
        for (i, (stage, status, summary, ratio, unlocked)) in states.iter().enumerate() {
            let ui = &mut cols[i];
            theme::card().show(ui, |ui| {
                ui.set_min_height(168.0);
                ui.horizontal(|ui| {
                    widgets::dot(ui, theme::stage_color(*stage), 10.0);
                    ui.label(
                        RichText::new(format!("{} {}", stage.ordinal(), stage.label()))
                            .strong()
                            .size(15.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::pill(ui, status.label(), theme::stage_status_color(*status));
                    });
                });
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new(stage.description())
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(theme::SPACE_SM);
                widgets::meter(ui, *ratio, theme::stage_color(*stage), ui.available_width());
                ui.label(RichText::new(summary).small().color(theme::TEXT_MUTED));
                ui.add_space(theme::SPACE_SM);

                ui.horizontal_wrapped(|ui| {
                    if !unlocked {
                        widgets::hint(ui, "上一阶段未审核");
                    } else if widgets::primary_button(ui, "运行", !busy) {
                        action = Some((*stage, StageAction::Run));
                    }
                    if widgets::button(ui, "打开", true) {
                        action = Some((*stage, StageAction::Open));
                    }
                    if status.is_approved() {
                        if widgets::button(ui, "撤销审核", !busy) {
                            action = Some((*stage, StageAction::Reset));
                        }
                    } else if widgets::button(ui, "审核通过", !busy) {
                        action = Some((*stage, StageAction::Approve));
                    }
                });
            });
        }
    });

    if let Some((stage, what)) = action {
        match what {
            StageAction::Open => cx.state.view = View::Stage(stage),
            StageAction::Run => crate::ui::app::run_stage(cx, stage),
            StageAction::Approve => cx.state.submit(cx.runtime, Job::Approve(stage)),
            StageAction::Reset => cx.state.submit(cx.runtime, Job::Reset(stage)),
        }
    }
}

#[derive(Clone, Copy)]
enum StageAction {
    Run,
    Open,
    Approve,
    Reset,
}

/// Preflight: everything that would otherwise fail halfway through a paid run.
fn readiness(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let Some(snapshot) = &cx.state.snapshot else {
        return;
    };
    let has_script = snapshot.script_path.is_some();
    let conflicts = snapshot.config.routing_conflicts();
    let credentials = cx.state.credentials();
    let lint = snapshot
        .breakdown
        .as_ref()
        .map(|b| b.lint())
        .unwrap_or_default();

    let mut rows: Vec<(bool, String)> = Vec::new();
    rows.push((
        has_script,
        if has_script {
            format!(
                "剧本已就绪：{}",
                snapshot
                    .script_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            )
        } else {
            "尚未导入剧本（剧本页可直接编写）".into()
        },
    ));

    for cap in Capability::ALL {
        let endpoint = snapshot.config.endpoint(cap);
        let supported = endpoint.provider.supports(cap);
        let has_key = credentials.has(cap, endpoint.provider, endpoint.mode);
        rows.push((
            supported && has_key,
            format!(
                "{}：{} · {}{}",
                cap.label(),
                endpoint.provider.label(),
                endpoint.model,
                if !supported {
                    "（该服务商不支持此能力）"
                } else if !has_key {
                    "（缺少密钥）"
                } else {
                    ""
                }
            ),
        ));
    }

    let mut goto_settings = false;
    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "运行前检查");
        ui.add_space(theme::SPACE_SM);
        for (ok, text) in &rows {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if *ok { "✔" } else { "!" })
                        .color(if *ok { theme::SUCCESS } else { theme::WARNING }),
                );
                ui.label(RichText::new(text).color(theme::TEXT_MUTED));
            });
        }
        if !conflicts.is_empty() || rows.iter().any(|(ok, _)| !ok) {
            ui.add_space(theme::SPACE_SM);
            if widgets::button(ui, "去设置", true) {
                goto_settings = true;
            }
        }

        if !lint.is_empty() {
            ui.add_space(theme::SPACE_MD);
            widgets::section_title(ui, "拆解提醒");
            for issue in lint.iter().take(6) {
                ui.label(RichText::new(format!("· {issue}")).small().color(theme::WARNING));
            }
            if lint.len() > 6 {
                widgets::hint(ui, &format!("还有 {} 条…", lint.len() - 6));
            }
        }
    });

    if goto_settings {
        cx.state.view = View::Settings;
    }
}

/// No project open: pick a recent one, browse, or create one.
fn welcome(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(
        ui,
        "欢迎使用 adrama",
        "剧本 → 拆解 → 资产 → 分镜 → 视频，每一步都可人工审核后再继续",
    );

    let recents = cx.state.settings.recent_projects.clone();
    let mut open_path: Option<std::path::PathBuf> = None;
    let mut forget: Option<std::path::PathBuf> = None;
    let mut create = false;

    ui.columns(2, |cols| {
        theme::card().show(&mut cols[0], |ui| {
            widgets::section_title(ui, "打开项目");
            ui.add_space(theme::SPACE_SM);
            if recents.is_empty() {
                widgets::hint(ui, "还没有最近项目。");
            }
            for path in recents.iter().take(8) {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string();
                let exists = crate::model::Project::is_project(path);
                let response = widgets::selectable_row(ui, false, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).color(if exists {
                            theme::TEXT
                        } else {
                            theme::TEXT_DIM
                        }));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !exists {
                                if ui.small_button("移除").clicked() {
                                    forget = Some(path.clone());
                                }
                                widgets::pill(ui, "已丢失", theme::DANGER);
                            }
                        });
                    });
                    widgets::path_label(ui, path);
                });
                if response.clicked() && exists {
                    open_path = Some(path.clone());
                }
            }
            ui.add_space(theme::SPACE_SM);
            if widgets::button(ui, "浏览文件夹…", true) {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    open_path = Some(path);
                }
            }
        });

        theme::card().show(&mut cols[1], |ui| {
            widgets::section_title(ui, "新建项目");
            ui.add_space(theme::SPACE_SM);
            let parent = cx
                .state
                .new_project
                .parent
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            widgets::field_row(ui, "位置", 56.0, |ui| {
                widgets::path_label(ui, &parent);
                if ui.small_button("更改").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        cx.state.new_project.parent = Some(p);
                    }
                }
            });
            widgets::field_row(ui, "名称", 56.0, |ui| {
                widgets::text_field(ui, &mut cx.state.new_project.name, 200.0, "my-drama");
            });
            widgets::field_row(ui, "风格", 56.0, |ui| {
                widgets::text_field(
                    ui,
                    &mut cx.state.new_project.style,
                    280.0,
                    "cinematic, photorealistic",
                );
            });
            widgets::field_row(ui, "画幅", 56.0, |ui| {
                for aspect in AspectRatio::ALL {
                    ui.selectable_value(
                        &mut cx.state.new_project.aspect,
                        aspect,
                        aspect.as_str(),
                    );
                }
            });
            ui.add_space(theme::SPACE_SM);
            if widgets::primary_button(ui, "创建项目", !cx.state.new_project.name.trim().is_empty())
            {
                create = true;
            }
        });
    });

    if let Some(path) = forget {
        cx.state.settings.forget_project(&path);
        let _ = cx.state.settings.save();
    }
    if let Some(path) = open_path {
        cx.state.open_project(cx.runtime, &path);
    }
    if create {
        let form = &cx.state.new_project;
        let name = form.name.trim().to_string();
        let path = form
            .parent
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(&name);
        let style = form.style.clone();
        let aspect = form.aspect;
        match Project::create(&path, &name, &style, aspect) {
            Ok(project) => {
                let root = project.root.clone();
                cx.state.note(format!("已创建项目 {}", root.display()));
                cx.state.open_project(cx.runtime, &root);
                cx.state.view = View::Script;
            }
            Err(err) => cx.state.fail(format!("创建失败：{err:#}")),
        }
    }
}
