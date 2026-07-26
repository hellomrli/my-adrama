//! Settings: provider credentials, capability routing, project parameters.

use eframe::egui::{self, RichText, Ui};

use super::ViewCtx;
use crate::engine::{Job, ProbeRequest};
use crate::model::{
    AspectRatio, Capability, EndpointMode, ProviderId,
};
use crate::settings::AppSettings;
use crate::ui::state::{SettingsTab, View};
use crate::ui::{theme, widgets};

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(ui, View::Settings.title(), View::Settings.subtitle());

    ui.horizontal(|ui| {
        for (tab, label) in [
            (SettingsTab::Providers, "服务商与密钥"),
            (SettingsTab::Routing, "能力路由"),
            (SettingsTab::Project, "项目参数"),
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
            SettingsTab::Providers => providers(ui, cx),
            SettingsTab::Routing => routing(ui, cx),
            SettingsTab::Project => project(ui, cx),
        });
}

fn providers(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let busy = cx.state.is_busy();
    let mut probe: Option<(ProviderId, EndpointMode)> = None;

    for id in ProviderId::ALL {
        theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::dot(ui, provider_color(id), 10.0);
                ui.label(RichText::new(id.label()).size(15.0).strong());
                ui.label(RichText::new(id.tagline()).small().color(theme::TEXT_MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for cap in Capability::ALL.into_iter().rev() {
                        if id.supports(cap) {
                            widgets::pill(ui, cap.label(), theme::SUCCESS);
                        } else {
                            widgets::pill(ui, cap.label(), theme::TEXT_DIM);
                        }
                    }
                });
            });

            ui.add_space(theme::SPACE_SM);
            widgets::field_row(ui, "端点模式", 84.0, |ui| {
                let settings = cx.state.config_draft.provider_mut(id);
                for mode in EndpointMode::ALL {
                    if ui
                        .selectable_value(&mut settings.mode, mode, mode.label())
                        .changed()
                    {
                        cx.state.config_dirty = true;
                    }
                }
            });

            // Official credentials
            ui.add_space(theme::SPACE_SM);
            widgets::hint(ui, &format!("官方地址 {}", id.official_base_url()));
            widgets::field_row(ui, "官方密钥", 84.0, |ui| {
                let mut key = cx.state.settings.key(id, EndpointMode::Official).to_string();
                let reveal_key = format!("{id}.official");
                let mut revealed = *cx.state.revealed.get(&reveal_key).unwrap_or(&false);
                if widgets::secret_field(ui, &mut key, &mut revealed, 280.0, "sk-… / AIza…") {
                    cx.state
                        .settings
                        .set_key(id, EndpointMode::Official, key.clone());
                    cx.state.keys_dirty = true;
                }
                cx.state.revealed.insert(reveal_key, revealed);
                if widgets::button(ui, "测试", !busy && !key.trim().is_empty()) {
                    probe = Some((id, EndpointMode::Official));
                }
            });

            // Custom endpoint
            ui.add_space(theme::SPACE_SM);
            widgets::field_row(ui, "自定义地址", 84.0, |ui| {
                let settings = cx.state.config_draft.provider_mut(id);
                if widgets::text_field(
                    ui,
                    &mut settings.custom_base_url,
                    280.0,
                    "https://proxy.example.com/v1",
                ) {
                    cx.state.config_dirty = true;
                }
            });
            widgets::field_row(ui, "自定义密钥", 84.0, |ui| {
                let mut key = cx.state.settings.key(id, EndpointMode::Custom).to_string();
                let reveal_key = format!("{id}.custom");
                let mut revealed = *cx.state.revealed.get(&reveal_key).unwrap_or(&false);
                if widgets::secret_field(ui, &mut key, &mut revealed, 280.0, "代理 / 自建服务密钥") {
                    cx.state
                        .settings
                        .set_key(id, EndpointMode::Custom, key.clone());
                    cx.state.keys_dirty = true;
                }
                cx.state.revealed.insert(reveal_key, revealed);
                let can_test = !busy
                    && !key.trim().is_empty()
                    && !cx
                        .state
                        .config_draft
                        .provider(id)
                        .custom_base_url
                        .trim()
                        .is_empty();
                if widgets::button(ui, "测试", can_test) {
                    probe = Some((id, EndpointMode::Custom));
                }
            });

            // Models per supported capability
            ui.add_space(theme::SPACE_SM);
            egui::CollapsingHeader::new(RichText::new("模型 ID").small())
                .id_salt(("models", id))
                .show(ui, |ui| {
                    for cap in Capability::ALL {
                        if !id.supports(cap) {
                            continue;
                        }
                        widgets::field_row(ui, cap.label(), 84.0, |ui| {
                            let settings = cx.state.config_draft.provider_mut(id);
                            if widgets::text_field(ui, settings.model_for_mut(cap), 260.0, "") {
                                cx.state.config_dirty = true;
                            }
                        });
                    }
                });
        });
        ui.add_space(theme::SPACE_SM);
    }

    save_bar(ui, cx);

    if let Some((id, mode)) = probe {
        let key = cx.state.settings.key(id, mode).to_string();
        let settings = cx.state.config_draft.provider(id).clone();
        let base_url = match mode {
            EndpointMode::Official => id.official_base_url().to_string(),
            EndpointMode::Custom => settings.custom_base_url.clone(),
        };
        cx.state.submit_probe(
            cx.runtime,
            Job::Probe(ProbeRequest {
                provider: id,
                mode,
                base_url,
                api_key: key,
                model: settings.chat_model.clone(),
            }),
        );
    }
}

fn routing(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    let credentials = cx.state.credentials();

    theme::card().show(ui, |ui| {
        widgets::section_title(ui, "每种能力由哪家服务商承担");
        widgets::hint(
            ui,
            "灰色表示该服务商不提供这项能力；选中它会在运行前直接报错，而不是发出错误请求。",
        );
        ui.add_space(theme::SPACE_MD);

        for cap in Capability::ALL {
            ui.horizontal(|ui| {
                ui.label(RichText::new(cap.label()).strong());
                ui.label(RichText::new(cap.description()).small().color(theme::TEXT_DIM));
            });
            ui.horizontal(|ui| {
                for id in ProviderId::ALL {
                    let supported = id.supports(cap);
                    let selected = cx.state.config_draft.routing.get(cap) == id;
                    let label = RichText::new(id.label()).color(if supported {
                        theme::TEXT
                    } else {
                        theme::TEXT_DIM
                    });
                    if ui
                        .add_enabled(supported, egui::SelectableLabel::new(selected, label))
                        .clicked()
                    {
                        cx.state.config_draft.routing.set(cap, id);
                        cx.state.config_dirty = true;
                    }
                }
            });

            let endpoint = cx.state.config_draft.endpoint(cap);
            let has_key = credentials.has(endpoint.provider, endpoint.mode);
            ui.horizontal_wrapped(|ui| {
                widgets::pill(ui, endpoint.mode.label(), theme::ACCENT);
                ui.label(
                    RichText::new(format!("{} · {}", endpoint.model, endpoint.base_url))
                        .small()
                        .monospace()
                        .color(theme::TEXT_DIM),
                );
                widgets::pill(
                    ui,
                    if has_key { "密钥已配置" } else { "缺少密钥" },
                    if has_key { theme::SUCCESS } else { theme::WARNING },
                );
            });
            ui.add_space(theme::SPACE_MD);
        }
    });

    ui.add_space(theme::SPACE_SM);
    save_bar(ui, cx);
}

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
        if changed {
            cx.state.config_dirty = true;
        }
    });

    ui.add_space(theme::SPACE_SM);
    save_bar(ui, cx);
}

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
                    "保存项目配置 *"
                } else {
                    "保存项目配置"
                },
                config_dirty && has_project,
            ) {
                save_config = true;
            }
            if widgets::button(ui, "放弃改动", config_dirty) {
                revert = true;
            }
            if !has_project {
                widgets::hint(ui, "项目配置需要先打开一个项目");
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

fn provider_color(id: ProviderId) -> egui::Color32 {
    match id {
        ProviderId::OpenAi => theme::SUCCESS,
        ProviderId::Google => theme::INFO,
        ProviderId::Xai => theme::WARNING,
    }
}
