//! 配音与字幕：逐镜头 TTS、SRT 字幕、成片合成选项、本地工具管理。

use eframe::egui::{self, RichText, Ui};

use super::ViewCtx;
use crate::engine::{stages, Job};
use crate::model::index::truncate;
use crate::tools::Tool;
use crate::ui::state::View;
use crate::ui::{theme, widgets};

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(ui, View::Audio.title(), View::Audio.subtitle());

    if cx.state.snapshot.is_none() {
        widgets::empty_state(ui, "尚未打开项目", "在「概览」中打开或新建一个项目。");
        return;
    }
    let has_breakdown = cx
        .state
        .snapshot
        .as_ref()
        .is_some_and(|s| s.breakdown.is_some());
    if !has_breakdown {
        widgets::empty_state(
            ui,
            "先完成「拆解」",
            "配音与字幕都来自拆解出的台词（dialogue）。",
        );
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            synth_settings(ui, cx);
            ui.add_space(theme::SPACE_SM);
            tools_card(ui, cx);
            ui.add_space(theme::SPACE_SM);
            voice_list(ui, cx);
        });
}

// ---------------------------------------------------------------------------
// 合成方式与成片选项
// ---------------------------------------------------------------------------

fn synth_settings(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "合成方式");
        ui.add_space(theme::SPACE_SM);

        widgets::field_row(ui, "引擎", 88.0, |ui| {
            let audio = &mut cx.state.config_draft.audio;
            let mut local = audio.local_tts;
            if ui
                .selectable_label(!local, "云端 API（配音模型）")
                .on_hover_text("走「设置 → 模型与密钥 → 配音模型」配置的端点")
                .clicked()
            {
                local = false;
            }
            if ui
                .selectable_label(local, "本地 Piper（离线免额度）")
                .on_hover_text("需要先在下方下载 Piper 引擎与中文音色")
                .clicked()
            {
                local = true;
            }
            if local != audio.local_tts {
                audio.local_tts = local;
                cx.state.config_dirty = true;
            }
        });

        let local = cx.state.config_draft.audio.local_tts;
        if !local {
            widgets::field_row(ui, "音色", 88.0, |ui| {
                let audio = &mut cx.state.config_draft.audio;
                if widgets::text_field(ui, &mut audio.voice, 160.0, "alloy") {
                    cx.state.config_dirty = true;
                }
                for preset in ["alloy", "nova", "onyx", "shimmer"] {
                    if ui.small_button(preset).clicked() {
                        cx.state.config_draft.audio.voice = preset.into();
                        cx.state.config_dirty = true;
                    }
                }
            });
        }

        ui.add_space(theme::SPACE_SM);
        widgets::section_title(ui, "成片选项（拼接时生效）");
        ui.add_space(theme::SPACE_XS);
        let audio = &mut cx.state.config_draft.audio;
        let mut changed = false;
        changed |= ui
            .checkbox(&mut audio.mix_voiceover, "把配音合成进音轨（会替换片段原声）")
            .changed();
        changed |= ui
            .checkbox(&mut audio.burn_subtitles, "把字幕烧录进画面（需重编码，稍慢）")
            .changed();
        if changed {
            cx.state.config_dirty = true;
        }

        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            if widgets::button(ui, "生成 SRT 字幕文件", true) {
                if let Some(root) = cx.state.root() {
                    match crate::engine::write_subtitles(&root) {
                        Ok((path, count)) => {
                            cx.state.note(format!("已生成 {count} 条字幕 → {}", path.display()))
                        }
                        Err(err) => cx.state.fail(format!("{err:#}")),
                    }
                }
            }
            let srt_exists = cx
                .state
                .root()
                .map(|r| r.join("video").join("subtitles.srt"))
                .filter(|p| p.is_file());
            if let Some(path) = srt_exists {
                if widgets::button(ui, "打开字幕文件", true) {
                    widgets::open_path(&path);
                }
            }
            widgets::hint(ui, "SRT 独立可用；「烧录」才会写死进画面");
        });
    });
}

// ---------------------------------------------------------------------------
// 本地工具（ffmpeg / Piper / 音色）
// ---------------------------------------------------------------------------

fn tools_card(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let probe = cx.state.tools();
    let installing = cx.state.tool_install;
    let mut install: Option<Tool> = None;

    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "本地工具");
        widgets::hint(
            ui,
            "从上游官方发布下载最新版，装进本机配置目录，不污染系统；已有系统安装则直接用。",
        );
        ui.add_space(theme::SPACE_SM);

        let row = |ui: &mut Ui,
                       tool: Tool,
                       status: Option<(String, bool)>,
                       needed: &str,
                       install: &mut Option<Tool>| {
            ui.horizontal(|ui| {
                let ok = status.is_some();
                ui.label(
                    RichText::new(if ok { "✔" } else { "○" })
                        .color(if ok { theme::SUCCESS } else { theme::TEXT_DIM }),
                );
                ui.label(RichText::new(tool.label()).strong());
                match &status {
                    Some((version, managed)) => {
                        widgets::pill(ui, if *managed { "托管" } else { "系统" }, theme::ACCENT);
                        ui.label(RichText::new(version).small().color(theme::TEXT_MUTED));
                    }
                    None => {
                        ui.label(RichText::new(needed).small().color(theme::TEXT_DIM));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match installing {
                        Some((active, received, total)) if active == tool => {
                            let ratio = if total > 0 {
                                received as f32 / total as f32
                            } else {
                                0.0
                            };
                            ui.add(
                                egui::ProgressBar::new(ratio)
                                    .desired_width(160.0)
                                    .text(format!("{:.0} MB", received as f64 / 1048576.0)),
                            );
                        }
                        _ => {
                            let label = if status.is_some() { "更新" } else { "下载" };
                            if widgets::button(ui, label, installing.is_none()) {
                                *install = Some(tool);
                            }
                        }
                    }
                });
            });
        };

        row(
            ui,
            Tool::Ffmpeg,
            probe.ffmpeg.as_ref().map(|s| (s.version.clone(), s.managed)),
            "拼接成片、混音、烧字幕需要",
            &mut install,
        );
        row(
            ui,
            Tool::Piper,
            probe.piper.as_ref().map(|s| (s.version.clone(), s.managed)),
            "本地配音需要（离线、免额度）",
            &mut install,
        );
        row(
            ui,
            Tool::PiperVoice,
            probe.piper_voice.as_ref().map(|p| {
                (
                    p.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                    true,
                )
            }),
            "Piper 的中文音色（zh_CN-huayan）",
            &mut install,
        );
    });

    if let Some(tool) = install {
        cx.state.tool_install = Some((tool, 0, 0));
        cx.state.note(format!("开始下载 {}（来自上游官方发布）", tool.label()));
        cx.runtime.install_tool(tool);
    }
}

// ---------------------------------------------------------------------------
// 逐镜头配音
// ---------------------------------------------------------------------------

fn voice_list(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let items = cx
        .state
        .snapshot
        .as_ref()
        .map(|s| s.voice.clone())
        .unwrap_or_default();
    let busy = cx.state.is_busy();
    let missing = items.iter().filter(|i| i.audio.is_none()).count();

    let mut job: Option<Job> = None;
    let mut upload_for: Option<String> = None;

    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::section_title(ui, &format!("逐镜头配音（{} 条台词）", items.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, &format!("生成全部缺失（{missing}）"), !busy && missing > 0)
                {
                    job = Some(Job::Voice(stages::voice::Selection::default()));
                }
            });
        });
        ui.add_space(theme::SPACE_SM);

        if items.is_empty() {
            widgets::hint(ui, "拆解里没有台词（dialogue 均为空），无从配音。");
        }
        for item in &items {
            theme::inset().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&item.shot_id)
                            .monospace()
                            .small()
                            .color(theme::ACCENT),
                    );
                    if item.audio.is_some() {
                        widgets::pill(ui, "已配音", theme::SUCCESS);
                    } else {
                        widgets::pill(ui, "未配音", theme::TEXT_DIM);
                    }
                    if item.manual {
                        widgets::pill(ui, "自备", theme::INFO);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::button(ui, "上传…", !busy) {
                            upload_for = Some(item.shot_id.clone());
                        }
                        if let Some(audio) = &item.audio {
                            let audio = audio.clone();
                            if widgets::button(ui, "播放", true) {
                                widgets::open_path(&audio);
                            }
                        }
                        let label = if item.audio.is_some() { "重配" } else { "生成" };
                        if widgets::button(ui, label, !busy) {
                            job = Some(Job::Voice(stages::voice::Selection::only(vec![
                                item.shot_id.clone(),
                            ])));
                        }
                    });
                });
                ui.label(
                    RichText::new(format!("「{}」", truncate(&item.dialogue, 60)))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        }
    });

    if let Some(id) = upload_for {
        upload_voice(cx, &id);
    }
    if let Some(job) = job {
        cx.state.submit(cx.runtime, job);
    }
}

fn upload_voice(cx: &mut ViewCtx<'_>, shot_id: &str) {
    let Some(root) = cx.state.root() else {
        return;
    };
    let Some(source) = rfd::FileDialog::new()
        .add_filter("音频", &["mp3", "wav"])
        .pick_file()
    else {
        return;
    };
    match crate::engine::import_voice_file(&root, shot_id, &source) {
        Ok(target) => {
            cx.state.note(format!("已导入配音 → {}", target.display()));
            cx.state.refresh(cx.runtime);
        }
        Err(err) => cx.state.fail(format!("导入失败：{err:#}")),
    }
}
