//! Stage 1 review — read the structured breakdown instead of raw JSON.

use eframe::egui::{self, RichText, Ui};

use super::ViewCtx;
use crate::engine::Job;
use crate::model::{index::truncate, Stage};
use crate::ui::state::View;
use crate::ui::{theme, widgets};

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(
        ui,
        View::Stage(Stage::Parse).title(),
        View::Stage(Stage::Parse).subtitle(),
    );

    if cx.state.snapshot.is_none() {
        widgets::empty_state(ui, "尚未打开项目", "在「概览」中打开或新建一个项目。");
        return;
    }

    toolbar(ui, cx);
    ui.add_space(theme::SPACE_SM);

    let has_breakdown = cx
        .state
        .snapshot
        .as_ref()
        .map(|s| s.breakdown.is_some())
        .unwrap_or(false);
    if !has_breakdown {
        widgets::empty_state(
            ui,
            "还没有拆解结果",
            "先在「剧本」页准备好文本，然后点击上方「运行拆解」。",
        );
        return;
    }

    if cx.state.raw_breakdown {
        raw_json(ui, cx);
    } else {
        structured(ui, cx);
    }
}

fn toolbar(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let busy = cx.state.is_busy();
    let status = cx
        .state
        .snapshot
        .as_ref()
        .map(|s| s.state.get(Stage::Parse))
        .unwrap_or_default();
    let path = cx
        .state
        .snapshot
        .as_ref()
        .map(|s| s.root.join(crate::model::project::BREAKDOWN_FILE));

    let mut job: Option<Job> = None;
    ui.horizontal_wrapped(|ui| {
        let label = if cx.state.dry_run {
            "运行拆解（演练）"
        } else {
            "运行拆解"
        };
        if widgets::primary_button(ui, label, !busy) {
            job = Some(Job::Parse);
        }
        if status.is_approved() {
            if widgets::button(ui, "撤销审核", !busy) {
                job = Some(Job::Reset(Stage::Parse));
            }
        } else if widgets::button(ui, "审核通过", !busy) {
            job = Some(Job::Approve(Stage::Parse));
        }
        widgets::pill(ui, status.label(), theme::stage_status_color(status));

        ui.separator();
        let mut raw = cx.state.raw_breakdown;
        if ui.selectable_label(raw, "原始 JSON").clicked() {
            raw = !raw;
        }
        cx.state.raw_breakdown = raw;

        if let Some(path) = path {
            if widgets::button(ui, "用编辑器打开", path.is_file()) {
                widgets::open_path(&path);
            }
        }
    });

    if let Some(job) = job {
        cx.state.submit(cx.runtime, job);
    }
}

fn structured(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let Some(snapshot) = &cx.state.snapshot else {
        return;
    };
    let Some(bd) = &snapshot.breakdown else {
        return;
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            theme::card().show(ui, |ui| {
                ui.label(RichText::new(&bd.title).size(17.0).strong());
                if !bd.summary.trim().is_empty() {
                    ui.label(RichText::new(&bd.summary).color(theme::TEXT_MUTED));
                }
                ui.add_space(theme::SPACE_SM);
                ui.horizontal_wrapped(|ui| {
                    widgets::pill(ui, &format!("{} 角色", bd.characters.len()), theme::ACCENT);
                    widgets::pill(ui, &format!("{} 场景", bd.locations.len()), theme::INFO);
                    widgets::pill(ui, &format!("{} 场", bd.scenes.len()), theme::INFO);
                    widgets::pill(ui, &format!("{} 镜头", bd.shots.len()), theme::SUCCESS);
                    widgets::pill(
                        ui,
                        &format!("约 {} 秒", bd.total_seconds()),
                        theme::TEXT_MUTED,
                    );
                });
            });

            let issues = bd.lint();
            if !issues.is_empty() {
                ui.add_space(theme::SPACE_SM);
                theme::inset().show(ui, |ui| {
                    widgets::section_title(ui, "需要注意");
                    for issue in issues.iter().take(10) {
                        ui.label(RichText::new(format!("· {issue}")).small().color(theme::WARNING));
                    }
                });
            }

            ui.add_space(theme::SPACE_MD);
            ui.columns(2, |cols| {
                theme::card().show(&mut cols[0], |ui| {
                    widgets::section_title(ui, &format!("角色（{}）", bd.characters.len()));
                    ui.add_space(theme::SPACE_XS);
                    for ch in &bd.characters {
                        theme::inset().show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&ch.name).strong());
                                ui.label(RichText::new(&ch.id).small().monospace().color(theme::TEXT_DIM));
                            });
                            ui.label(RichText::new(&ch.appearance).small().color(theme::TEXT_MUTED));
                            if !ch.costume.trim().is_empty() {
                                ui.label(
                                    RichText::new(format!("服装：{}", ch.costume))
                                        .small()
                                        .color(theme::TEXT_DIM),
                                );
                            }
                        });
                    }
                    if bd.characters.is_empty() {
                        widgets::hint(ui, "没有角色");
                    }

                    if !bd.locations.is_empty() {
                        ui.add_space(theme::SPACE_MD);
                        widgets::section_title(ui, &format!("场景（{}）", bd.locations.len()));
                        for loc in &bd.locations {
                            ui.label(RichText::new(format!(
                                "· {} — {}",
                                loc.name,
                                truncate(&loc.description, 48)
                            ))
                            .small()
                            .color(theme::TEXT_MUTED));
                        }
                    }
                });

                theme::card().show(&mut cols[1], |ui| {
                    widgets::section_title(ui, &format!("镜头表（{}）", bd.shots.len()));
                    ui.add_space(theme::SPACE_XS);
                    for scene in &bd.scenes {
                        let shots = bd.shots_in_scene(scene.number);
                        egui::CollapsingHeader::new(
                            RichText::new(format!(
                                "第 {} 场 · {}（{} 镜）",
                                scene.number,
                                scene.title,
                                shots.len()
                            ))
                            .strong(),
                        )
                        .default_open(true)
                        .show(ui, |ui| {
                            for shot in shots {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new(&shot.id).small().monospace().color(theme::ACCENT),
                                    );
                                    widgets::pill(ui, &shot.framing, theme::TEXT_MUTED);
                                    ui.label(
                                        RichText::new(format!("{}s", shot.duration_secs))
                                            .small()
                                            .color(theme::TEXT_DIM),
                                    );
                                });
                                ui.label(
                                    RichText::new(truncate(&shot.visual, 110))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                                if !shot.dialogue.trim().is_empty() {
                                    ui.label(
                                        RichText::new(format!("「{}」", truncate(&shot.dialogue, 60)))
                                            .small()
                                            .color(theme::TEXT_DIM),
                                    );
                                }
                                ui.add_space(theme::SPACE_XS);
                            }
                        });
                    }

                    // Shots whose scene is missing would otherwise be invisible.
                    let orphans: Vec<_> = bd
                        .shots
                        .iter()
                        .filter(|s| bd.scene(&s.scene_id).is_none())
                        .collect();
                    if !orphans.is_empty() {
                        ui.add_space(theme::SPACE_SM);
                        widgets::section_title(ui, "未归入场次的镜头");
                        for shot in orphans {
                            ui.label(
                                RichText::new(format!("· {} → {}", shot.id, shot.scene_id))
                                    .small()
                                    .color(theme::WARNING),
                            );
                        }
                    }
                });
            });
        });
}

fn raw_json(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let mut text = cx
        .state
        .snapshot
        .as_ref()
        .map(|s| s.breakdown_json.clone())
        .unwrap_or_default();

    theme::card().show(ui, |ui| {
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(28)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            });
    });
}
