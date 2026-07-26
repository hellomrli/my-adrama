//! 设置。
//!
//! 密钥这一块按**能力**组织，而不是按服务商：一张卡回答「这件事找谁做、
//! 连哪里、用什么密钥、跑哪个模型」。端点与密钥在服务商层面是共用的，
//! 所以两种能力选了同一家时会明确提示，避免改了一处影响另一处却不自知。

use eframe::egui::{self, RichText, Ui};

use super::ViewCtx;
use crate::engine::{Job, ProbeRequest};
use crate::model::{AspectRatio, Capability, EndpointMode, ProviderId};
use crate::providers::looks_like;
use crate::settings::AppSettings;
use crate::ui::state::{SettingsTab, View};
use crate::ui::{theme, widgets};
use crate::update::{self, UpdateStatus};

const LABEL_W: f32 = 88.0;

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(ui, View::Settings.title(), View::Settings.subtitle());

    ui.horizontal(|ui| {
        for (tab, label) in [
            (SettingsTab::Models, "模型与密钥"),
            (SettingsTab::Project, "项目参数"),
            (SettingsTab::About, "关于与更新"),
        ] {
            if ui
                .selectable_label(cx.state.settings_tab == tab, label)
                .clicked()
            {
                cx.state.settings_tab = tab;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::hint(
                ui,
                &format!("密钥文件 {}", AppSettings::config_path().display()),
            );
        });
    });
    ui.add_space(theme::SPACE_MD);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match cx.state.settings_tab {
            SettingsTab::Models => capabilities(ui, cx),
            SettingsTab::Project => project(ui, cx),
            SettingsTab::About => about(ui, cx),
        });
}

// ---------------------------------------------------------------------------
// 模型与密钥：一种能力一张卡，彼此完全独立
// ---------------------------------------------------------------------------

fn capabilities(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::hint(
        ui,
        "三种能力各自独立：服务商、地址、密钥、模型都分开存，改一处不会动到另一处。",
    );
    ui.add_space(theme::SPACE_SM);

    let mut probe: Option<Capability> = None;
    let mut copy_from: Option<(Capability, Capability)> = None;

    for cap in Capability::ALL {
        let action = capability_card(ui, cx, cap);
        if action.probe {
            probe = Some(cap);
        }
        if let Some(source) = action.copy_key_from {
            copy_from = Some((source, cap));
        }
        ui.add_space(theme::SPACE_SM);
    }

    save_bar(ui, cx);

    if let Some((from, to)) = copy_from {
        copy_key(cx, from, to);
    }
    if let Some(cap) = probe {
        launch_probe(cx, cap);
    }
}

#[derive(Default)]
struct CardAction {
    probe: bool,
    /// 从哪种能力把密钥抄过来（一次性复制，不是共享）。
    copy_key_from: Option<Capability>,
}

fn capability_card(ui: &mut Ui, cx: &mut ViewCtx<'_>, cap: Capability) -> CardAction {
    let busy = cx.state.is_busy();
    let slot = cx.state.config_draft.slot(cap).clone();
    let has_key = !cx
        .state
        .settings
        .key(cap, slot.provider, slot.mode)
        .is_empty();
    let mut action = CardAction::default();

    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::dot(ui, capability_color(cap), 10.0);
            ui.label(RichText::new(capability_title(cap)).size(15.0).strong());
            ui.label(
                RichText::new(cap.description())
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = has_key && !slot.model.trim().is_empty();
                widgets::pill(
                    ui,
                    if ready { "可用" } else { "未配置完整" },
                    if ready { theme::SUCCESS } else { theme::WARNING },
                );
            });
        });

        // 1. 服务商
        ui.add_space(theme::SPACE_SM);
        widgets::field_row(ui, "服务商", LABEL_W, |ui| {
            for id in ProviderId::ALL {
                let supported = id.supports(cap);
                let selected = slot.provider == id;
                let text = RichText::new(id.label()).color(if supported {
                    theme::TEXT
                } else {
                    theme::TEXT_DIM
                });
                let response = ui
                    .add_enabled(supported, egui::SelectableLabel::new(selected, text))
                    .on_hover_text(id.tagline())
                    .on_disabled_hover_text(format!("{} 不提供{}能力", id.label(), cap.label()));
                if response.clicked() && !selected {
                    cx.state.config_draft.slot_mut(cap).switch_provider(id, cap);
                    cx.state.config_dirty = true;
                }
            }
        });

        // 2. 端点
        widgets::field_row(ui, "端点", LABEL_W, |ui| {
            let entry = cx.state.config_draft.slot_mut(cap);
            for m in EndpointMode::ALL {
                if ui.selectable_value(&mut entry.mode, m, m.label()).changed() {
                    cx.state.config_dirty = true;
                }
            }
            if slot.mode == EndpointMode::Official {
                ui.label(
                    RichText::new(slot.provider.official_base_url())
                        .small()
                        .monospace()
                        .color(theme::TEXT_DIM),
                );
            }
        });
        if slot.mode == EndpointMode::Custom {
            widgets::field_row(ui, "地址", LABEL_W, |ui| {
                let entry = cx.state.config_draft.slot_mut(cap);
                if widgets::text_field(
                    ui,
                    &mut entry.custom_base_url,
                    320.0,
                    "https://proxy.example.com/v1",
                ) {
                    cx.state.config_dirty = true;
                }
            });
        }

        // 3. 密钥
        widgets::field_row(ui, "密钥", LABEL_W, |ui| {
            let mut key = cx
                .state
                .settings
                .key(cap, slot.provider, slot.mode)
                .to_string();
            let reveal_key = format!("{cap}.{}.{}", slot.provider, slot.mode.label());
            let mut revealed = *cx.state.revealed.get(&reveal_key).unwrap_or(&false);
            if widgets::secret_field(ui, &mut key, &mut revealed, 300.0, key_hint(slot.provider)) {
                cx.state
                    .settings
                    .set_key(cap, slot.provider, slot.mode, key.clone());
                cx.state.keys_dirty = true;
            }
            cx.state.revealed.insert(reveal_key, revealed);

            let url_ready =
                slot.mode == EndpointMode::Official || !slot.custom_base_url.trim().is_empty();
            if ui
                .add_enabled(
                    !busy && !key.trim().is_empty() && url_ready,
                    egui::Button::new("测试并拉取模型"),
                )
                .on_hover_text("验证这一格的密钥，并拉取该端点当前提供的模型")
                .clicked()
            {
                action.probe = true;
            }
        });

        // 同一个端点在别处填过密钥时，给一个一次性复制的入口（不是共享）
        if !has_key {
            if let Some(source) = donor_capability(cx, cap, &slot) {
                widgets::field_row(ui, "", LABEL_W, |ui| {
                    if ui
                        .small_button(format!("沿用「{}」的密钥", capability_title(source)))
                        .on_hover_text("复制一份过来，之后两边各自独立，改一边不影响另一边")
                        .clicked()
                    {
                        action.copy_key_from = Some(source);
                    }
                });
            }
        }

        // 4. 模型
        model_row(ui, cx, cap, &slot);
    });

    action
}

/// 哪种能力用着同样的服务商+端点模式且已经填了密钥。
fn donor_capability(
    cx: &ViewCtx<'_>,
    cap: Capability,
    slot: &crate::model::EndpointConfig,
) -> Option<Capability> {
    Capability::ALL.into_iter().find(|other| {
        if *other == cap {
            return false;
        }
        let theirs = cx.state.config_draft.slot(*other);
        theirs.provider == slot.provider
            && theirs.mode == slot.mode
            && theirs.custom_base_url.trim() == slot.custom_base_url.trim()
            && !cx
                .state
                .settings
                .key(*other, theirs.provider, theirs.mode)
                .is_empty()
    })
}

fn copy_key(cx: &mut ViewCtx<'_>, from: Capability, to: Capability) {
    let source = cx.state.config_draft.slot(from).clone();
    let key = cx
        .state
        .settings
        .key(from, source.provider, source.mode)
        .to_string();
    if key.is_empty() {
        return;
    }
    let target = cx.state.config_draft.slot(to).clone();
    cx.state
        .settings
        .set_key(to, target.provider, target.mode, key);
    // 模型列表也一并带过来，省一次探测。
    let models = cx
        .state
        .settings
        .known_models(from, source.provider, source.mode)
        .to_vec();
    if !models.is_empty() {
        cx.state
            .settings
            .set_known_models(to, target.provider, target.mode, models);
    }
    cx.state.keys_dirty = true;
    cx.state.note(format!(
        "已把「{}」的密钥复制到「{}」，两者之后互不影响",
        capability_title(from),
        capability_title(to)
    ));
}

fn model_row(ui: &mut Ui, cx: &mut ViewCtx<'_>, cap: Capability, slot: &crate::model::EndpointConfig) {
    let known: Vec<String> = cx
        .state
        .settings
        .known_models(cap, slot.provider, slot.mode)
        .to_vec();
    let current = slot.model.clone();

    widgets::field_row(ui, "模型", LABEL_W, |ui| {
        let entry = cx.state.config_draft.slot_mut(cap);
        if widgets::text_field(ui, &mut entry.model, 240.0, "模型 ID") {
            cx.state.config_dirty = true;
        }

        if known.is_empty() {
            widgets::hint(ui, "点「测试并拉取模型」后可从列表选择");
            return;
        }

        // 与本能力相关的排前面，其余照列——厂商改名太频繁，隐藏比列出更危险。
        let (likely, others): (Vec<&String>, Vec<&String>) =
            known.iter().partition(|m| looks_like(cap, m));
        let mut picked: Option<String> = None;

        egui::ComboBox::from_id_salt(("model_pick", cap))
            .selected_text(format!("从 {} 个中选择", known.len()))
            .width(150.0)
            .show_ui(ui, |ui| {
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    if likely.is_empty() {
                        widgets::hint(ui, "没有明显匹配的，以下是全部");
                    }
                    for model in &likely {
                        if ui.selectable_label(**model == current, *model).clicked() {
                            picked = Some((*model).clone());
                        }
                    }
                    if !others.is_empty() {
                        ui.separator();
                        widgets::hint(ui, "其它模型");
                        for model in &others {
                            if ui.selectable_label(**model == current, *model).clicked() {
                                picked = Some((*model).clone());
                            }
                        }
                    }
                });
            });

        if let Some(model) = picked {
            cx.state.config_draft.slot_mut(cap).model = model;
            cx.state.config_dirty = true;
        }

        if !current.trim().is_empty() && !known.iter().any(|m| m == &current) {
            widgets::pill(ui, "不在列表中", theme::WARNING)
                .on_hover_text("上游没有列出这个模型；可能已下线，或代理未返回全部模型");
        }
    });
}

fn launch_probe(cx: &mut ViewCtx<'_>, cap: Capability) {
    let slot = cx.state.config_draft.slot(cap).clone();
    let key = cx
        .state
        .settings
        .key(cap, slot.provider, slot.mode)
        .to_string();
    cx.state.submit_probe(
        cx.runtime,
        Job::Probe(ProbeRequest {
            capability: cap,
            provider: slot.provider,
            mode: slot.mode,
            base_url: slot.base_url(),
            api_key: key,
            model: slot.model.clone(),
        }),
    );
}

fn capability_title(cap: Capability) -> &'static str {
    match cap {
        Capability::Chat => "对话模型",
        Capability::Image => "生图模型",
        Capability::Video => "视频模型",
        Capability::Speech => "配音模型",
    }
}

fn capability_color(cap: Capability) -> egui::Color32 {
    match cap {
        Capability::Chat => theme::stage_color(crate::model::Stage::Parse),
        Capability::Image => theme::stage_color(crate::model::Stage::Assets),
        Capability::Video => theme::stage_color(crate::model::Stage::Video),
        Capability::Speech => theme::INFO,
    }
}

fn key_hint(id: ProviderId) -> &'static str {
    match id {
        ProviderId::OpenAi => "sk-…",
        ProviderId::Google => "AIza…",
        ProviderId::Xai => "xai-…",
    }
}

// ---------------------------------------------------------------------------
// 项目参数
// ---------------------------------------------------------------------------

fn project(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    if cx.state.snapshot.is_none() {
        widgets::empty_state(ui, "尚未打开项目", "打开项目后可编辑 project.toml。");
        return;
    }

    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "基本信息");
        ui.add_space(theme::SPACE_SM);
        widgets::field_row(ui, "名称", 84.0, |ui| {
            if widgets::text_field(ui, &mut cx.state.config_draft.name, 240.0, "") {
                cx.state.config_dirty = true;
            }
        });
        widgets::field_row(ui, "风格前缀", 84.0, |ui| {
            if widgets::text_field(ui, &mut cx.state.config_draft.style, 380.0, "") {
                cx.state.config_dirty = true;
            }
        });
        widgets::field_row(ui, "画幅", 84.0, |ui| {
            for aspect in AspectRatio::ALL {
                if ui
                    .selectable_value(&mut cx.state.config_draft.aspect, aspect, aspect.as_str())
                    .changed()
                {
                    cx.state.config_dirty = true;
                }
            }
        });
    });

    ui.add_space(theme::SPACE_SM);
    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "生成参数");
        widgets::hint(ui, "视频最贵，默认串行；图像可以并发以缩短等待。");
        ui.add_space(theme::SPACE_SM);

        let gen = &mut cx.state.config_draft.generation;
        let mut changed = false;
        widgets::field_row(ui, "图像并发", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.image_concurrency, 1..=8))
                .changed();
        });
        widgets::field_row(ui, "视频并发", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.video_concurrency, 1..=4))
                .changed();
        });
        widgets::field_row(ui, "单镜头上限", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.max_shot_seconds, 2..=16).suffix(" 秒"))
                .changed();
        });
        widgets::field_row(ui, "轮询间隔", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.video_poll_secs, 2..=60).suffix(" 秒"))
                .changed();
        });
        widgets::field_row(ui, "视频超时", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.video_timeout_secs, 60..=7200).suffix(" 秒"))
                .changed();
        });
        widgets::field_row(ui, "重试次数", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.request_retries, 1..=6))
                .changed();
        });
        widgets::field_row(ui, "每镜分镜帧数", 96.0, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut gen.frames_per_shot, 2..=8).suffix(" 帧"))
                .on_hover_text("首帧…末帧；末帧与下一镜首帧同源，片段之间才接得上。可在分镜页按镜头单独覆盖")
                .changed();
        });
        if changed {
            cx.state.config_dirty = true;
        }
    });

    ui.add_space(theme::SPACE_SM);
    save_bar(ui, cx);
}

// ---------------------------------------------------------------------------
// 关于与更新
// ---------------------------------------------------------------------------

fn about(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let checking = cx.state.updates.checking;
    let downloading = cx.state.updates.download;
    let can_self_update = cx.state.updates.install.can_self_update();
    let install_desc = cx.state.updates.install.describe();

    let mut do_check = false;
    let mut do_apply = false;
    let mut do_restart = false;
    let mut open_page: Option<String> = None;

    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::section_title(ui, "adrama");
            widgets::pill(ui, update::CURRENT_VERSION, theme::ACCENT);
        });
        widgets::hint(ui, &install_desc);
        ui.add_space(theme::SPACE_SM);

        ui.horizontal_wrapped(|ui| {
            if widgets::primary_button(ui, "检查更新", !checking && downloading.is_none()) {
                do_check = true;
            }
            if checking {
                ui.spinner();
                widgets::hint(ui, "正在查询 GitHub…");
            }
            let mut auto = cx.state.settings.ui.auto_check_updates;
            if ui.checkbox(&mut auto, "启动时自动检查").changed() {
                cx.state.settings.ui.auto_check_updates = auto;
                let _ = cx.state.settings.save();
            }
            if widgets::button(ui, "打开发布页", true) {
                open_page = Some(format!("https://github.com/{}/releases", update::REPO));
            }
        });

        if let Some((received, total)) = downloading {
            ui.add_space(theme::SPACE_SM);
            let ratio = if total > 0 {
                received as f32 / total as f32
            } else {
                0.0
            };
            ui.add(
                egui::ProgressBar::new(ratio)
                    .desired_width(320.0)
                    .text(format!("{} / {}", human_size(received), human_size(total))),
            );
        }

        if let Some(applied) = &cx.state.updates.applied {
            ui.add_space(theme::SPACE_SM);
            theme::inset().show(ui, |ui| {
                ui.label(
                    RichText::new(format!("已更新到 {}，重启后生效", applied.version))
                        .color(theme::SUCCESS),
                );
                widgets::hint(
                    ui,
                    if applied.verified {
                        "已通过 SHA-256 校验"
                    } else {
                        "该 release 未提供校验和文件，仅依赖 HTTPS"
                    },
                );
                if widgets::primary_button(ui, "立即重启", true) {
                    do_restart = true;
                }
            });
        }

        ui.add_space(theme::SPACE_SM);
        match &cx.state.updates.last_result {
            None => widgets::hint(ui, "还没有检查过更新。"),
            Some(Err(err)) => {
                ui.label(RichText::new(format!("检查失败：{err}")).color(theme::DANGER));
            }
            Some(Ok(UpdateStatus::UpToDate)) => {
                ui.label(
                    RichText::new(format!("已是最新版本（{}）", update::CURRENT_VERSION))
                        .color(theme::SUCCESS),
                );
            }
            Some(Ok(UpdateStatus::Available(release))) => {
                let has_asset = release.asset_for_platform().is_some();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("发现新版本 {}", release.version))
                            .strong()
                            .color(theme::WARNING),
                    );
                    widgets::pill(ui, &release.tag, theme::WARNING);
                });

                if !release.notes.trim().is_empty() {
                    ui.add_space(theme::SPACE_XS);
                    theme::inset().show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .id_salt("release_notes")
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&release.notes)
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                            });
                    });
                }

                ui.add_space(theme::SPACE_SM);
                ui.horizontal_wrapped(|ui| {
                    if can_self_update && has_asset {
                        if widgets::primary_button(
                            ui,
                            "下载并安装",
                            downloading.is_none() && cx.state.updates.applied.is_none(),
                        ) {
                            do_apply = true;
                        }
                    } else if !has_asset {
                        widgets::hint(
                            ui,
                            &format!(
                                "该版本没有当前平台的产物（{}）",
                                update::platform_asset_name()
                            ),
                        );
                    } else {
                        widgets::hint(ui, "此副本由包管理器安装，请用 apt / dpkg 更新");
                    }
                    if widgets::button(ui, "查看发布页", true) {
                        open_page = Some(release.page_url.clone());
                    }
                });
            }
        }
    });

    if do_check {
        cx.state.start_update_check(cx.runtime);
    }
    if do_apply {
        cx.state.start_update_download(cx.runtime);
    }
    if do_restart {
        cx.state.restart_after_update(ui.ctx());
    }
    if let Some(url) = open_page {
        widgets::open_url(&url);
    }

    ui.add_space(theme::SPACE_SM);
    self_check(ui, cx);
}

/// 出问题时的一站式现场信息：把该看的都摆出来，省得来回问。
fn self_check(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let report = diagnostics(cx);
    let mut run_check = false;
    let mut copy = false;

    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "自检");
        widgets::hint(ui, "遇到「点了没反应」时，先看这里，再把内容发出来。");
        ui.add_space(theme::SPACE_SM);

        for (ok, line) in &report.rows {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if *ok { "✔" } else { "!" })
                        .color(if *ok { theme::SUCCESS } else { theme::WARNING }),
                );
                ui.label(RichText::new(line).small().color(theme::TEXT_MUTED));
            });
        }

        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            if widgets::primary_button(ui, "运行自检（演练，不花钱）", !cx.state.is_busy()) {
                run_check = true;
            }
            if widgets::button(ui, "复制诊断信息", true) {
                copy = true;
            }
            if widgets::button(ui, "打开日志文件", AppSettings::log_path().is_file()) {
                widgets::open_path(&AppSettings::log_path());
            }
        });
        widgets::hint(ui, &format!("日志：{}", AppSettings::log_path().display()));
    });

    if run_check {
        cx.state.console_open = true;
        cx.state.push_console(
            crate::engine::events::Level::Info,
            "自检开始：验证项目、剧本与配置能否走通，只组装 prompt，不调用模型、不产生费用",
        );
        let was_dry = cx.state.dry_run;
        cx.state.dry_run = true;
        cx.state.submit(cx.runtime, Job::Parse);
        cx.state.dry_run = was_dry;
        cx.state.push_console(
            crate::engine::events::Level::Warn,
            "自检不会真的拆解剧本。要真跑：左侧「解析」页 →「运行拆解」（按钮上不带「演练」字样）",
        );
    }
    if copy {
        ui.ctx().copy_text(report.text);
        cx.state.note("诊断信息已复制到剪贴板");
    }
}

struct Diagnostics {
    rows: Vec<(bool, String)>,
    text: String,
}

fn diagnostics(cx: &ViewCtx<'_>) -> Diagnostics {
    let tool_probe = cx.state.tool_cache.clone();
    let state = &cx.state;
    let mut rows: Vec<(bool, String)> = Vec::new();

    rows.push((
        true,
        format!(
            "版本 {} · {}",
            update::CURRENT_VERSION,
            state.updates.install.describe()
        ),
    ));
    rows.push((
        !state.dry_run,
        if state.dry_run {
            "演练模式开着：只会展示 prompt，不会调用模型".into()
        } else {
            "演练模式已关闭".to_string()
        },
    ));
    rows.push((
        !state.is_busy(),
        if state.is_busy() {
            "有任务正在运行，运行按钮此时是禁用的".into()
        } else {
            "后台空闲，可以提交任务".to_string()
        },
    ));

    match &state.snapshot {
        None => rows.push((false, "尚未打开项目".into())),
        Some(snapshot) => {
            rows.push((true, format!("项目 {}", snapshot.root.display())));
            match &snapshot.script_path {
                Some(path) => rows.push((
                    true,
                    format!(
                        "剧本 {}（{} 字）",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        snapshot.script_text.chars().count()
                    ),
                )),
                None => rows.push((false, "script/ 下没有剧本文件，拆解无从开始".into())),
            }
        }
    }

    let credentials = state.credentials();
    for cap in Capability::ALL {
        let endpoint = state.config_draft.endpoint(cap);
        let has_key = credentials.has(cap, endpoint.provider, endpoint.mode);
        let supported = endpoint.provider.supports(cap);
        rows.push((
            has_key && supported && !endpoint.model.is_empty(),
            format!(
                "{}：{} · {} · {} · {}",
                capability_title(cap),
                endpoint.provider.label(),
                endpoint.mode.label(),
                if endpoint.model.is_empty() {
                    "未设置模型"
                } else {
                    &endpoint.model
                },
                if !supported {
                    "该服务商不提供此能力"
                } else if has_key {
                    "密钥已配置"
                } else {
                    "缺少密钥"
                }
            ),
        ));
        rows.push((true, format!("    地址 {}", endpoint.base_url)));
    }

    match &tool_probe {
        Some(probe) => {
            rows.push((
                probe.ffmpeg.is_some(),
                match &probe.ffmpeg {
                    Some(s) => format!("ffmpeg：{}（{}）", s.version, if s.managed { "托管" } else { "系统" }),
                    None => "ffmpeg：未安装（拼接成片需要；「配音与字幕」页可下载）".into(),
                },
            ));
            rows.push((
                true,
                match (&probe.piper, &probe.piper_voice) {
                    (Some(_), Some(_)) => "本地 TTS：Piper + 中文音色已就绪".into(),
                    (Some(_), None) => "本地 TTS：Piper 已装，缺中文音色（可选）".into(),
                    _ => "本地 TTS：未安装（可选）".into(),
                },
            ));
        }
        None => rows.push((true, "本地工具：尚未探测（打开「配音与字幕」页会自动探测）".into())),
    }

    if state.config_dirty || state.keys_dirty {
        rows.push((false, "有改动尚未保存（运行任务时会自动保存）".into()));
    }

    let mut text = String::from("adrama 诊断信息\n");
    for (ok, line) in &rows {
        text.push_str(&format!("{} {line}\n", if *ok { "[ok]" } else { "[!]" }));
    }
    text.push_str("\n最近日志：\n");
    for line in state
        .console
        .iter()
        .rev()
        .take(25)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        text.push_str(&format!("  {}\n", line.text));
    }

    Diagnostics { rows, text }
}

fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

// ---------------------------------------------------------------------------

fn save_bar(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let keys_dirty = cx.state.keys_dirty;
    let config_dirty = cx.state.config_dirty;
    let has_project = cx.state.snapshot.is_some();

    let mut save_keys = false;
    let mut save_config = false;
    let mut revert = false;

    theme::inset().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::primary_button(
                ui,
                if keys_dirty { "保存密钥 *" } else { "保存密钥" },
                keys_dirty,
            ) {
                save_keys = true;
            }
            if widgets::primary_button(
                ui,
                if config_dirty {
                    "保存到项目 *"
                } else {
                    "保存到项目"
                },
                config_dirty && has_project,
            ) {
                save_config = true;
            }
            if widgets::button(ui, "放弃改动", config_dirty) {
                revert = true;
            }
            if has_project {
                widgets::hint(ui, "供应商、端点与模型属于项目配置，写入 project.toml；密钥只存本机");
            } else {
                widgets::hint(ui, "供应商与模型需要先打开一个项目才能保存");
            }
        });
    });

    if save_keys {
        cx.state.save_keys();
    }
    if save_config {
        cx.state.save_project_config(cx.runtime);
    }
    if revert {
        if let Some(snapshot) = &cx.state.snapshot {
            cx.state.config_draft = snapshot.config.clone();
        }
        cx.state.config_dirty = false;
    }
}
