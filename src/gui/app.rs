use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText, Sense, TextureHandle, Vec2};

use super::theme;
use super::worker::{Job, WorkerHandle, WorkerMsg};
use super::workflow::{WfAction, WfNodeKind, WorkflowCanvas};
use crate::model::{Breakdown, ItemStatus};
use crate::project::{
    EndpointMode, Project, ProjectConfig, ProviderKind, Stage, StageStatus, GOOGLE_OFFICIAL_BASE,
    OPENAI_OFFICIAL_BASE, XAI_OFFICIAL_BASE,
};
use crate::settings::AppSettings;
use crate::stages;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Home,
    Workflow,
    Script,
    Parse,
    Assets,
    Storyboard,
    Video,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsPane {
    Keys,
    Routing,
    Project,
}

pub struct AdramaApp {
    worker: WorkerHandle,
    project: Option<Project>,
    tab: Tab,
    settings_pane: SettingsPane,
    busy: bool,
    busy_label: String,
    busy_kind: Option<WfNodeKind>,
    progress: Option<(u32, u32, String)>,
    logs: Vec<String>,
    status_msg: String,
    error_msg: String,
    preview_image: Option<PathBuf>,
    script_path: Option<PathBuf>,
    script_dirty: bool,

    new_name: String,
    new_style: String,
    new_aspect: String,
    create_parent: PathBuf,

    dry_run: bool,
    only_asset: String,
    scene_filter: String,
    shot_filter: String,
    video_concat: bool,
    redo_id: String,

    app_settings: AppSettings,
    edit_config: ProjectConfig,
    settings_dirty: bool,

    workflow: WorkflowCanvas,
    textures: HashMap<PathBuf, TextureHandle>,
    last_refresh: Instant,
    refresh_interval: Duration,

    breakdown_text: String,
    script_text: String,
    asset_thumbs: Vec<PathBuf>,
    storyboard_thumbs: Vec<PathBuf>,
    video_files: Vec<PathBuf>,
}

impl AdramaApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, initial: PathBuf) -> Self {
        let mut app_settings = AppSettings::load();
        app_settings.merge_from_env();
        app_settings.apply_to_env();

        let mut app = Self {
            worker: WorkerHandle::spawn(),
            project: None,
            tab: Tab::Home,
            settings_pane: SettingsPane::Keys,
            busy: false,
            busy_label: String::new(),
            busy_kind: None,
            progress: None,
            logs: Vec::new(),
            status_msg: "欢迎使用 my-adrama".into(),
            error_msg: String::new(),
            preview_image: None,
            script_path: None,
            script_dirty: false,
            new_name: "my-drama".into(),
            new_style: "cinematic, photorealistic, film grain".into(),
            new_aspect: "16:9".into(),
            create_parent: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            dry_run: false,
            only_asset: String::new(),
            scene_filter: String::new(),
            shot_filter: String::new(),
            video_concat: false,
            redo_id: String::new(),
            app_settings,
            edit_config: ProjectConfig::default(),
            settings_dirty: false,
            workflow: WorkflowCanvas::default(),
            textures: HashMap::new(),
            last_refresh: Instant::now() - Duration::from_secs(60),
            refresh_interval: Duration::from_secs(2),
            breakdown_text: String::new(),
            script_text: String::new(),
            asset_thumbs: Vec::new(),
            storyboard_thumbs: Vec::new(),
            video_files: Vec::new(),
        };

        let try_path = if initial.join("project.toml").exists() {
            Some(initial)
        } else if let Some(last) = app.app_settings.last_project.clone() {
            last.join("project.toml").exists().then_some(last)
        } else if initial == Path::new(".") {
            std::env::current_dir()
                .ok()
                .filter(|p| p.join("project.toml").exists())
        } else {
            None
        };
        if let Some(p) = try_path {
            app.open_project(&p);
        }
        app
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.logs.push(line.into());
        if self.logs.len() > 500 {
            let n = self.logs.len() - 500;
            self.logs.drain(0..n);
        }
    }

    fn open_project(&mut self, path: &Path) {
        match Project::open(path) {
            Ok(proj) => {
                self.edit_config = proj.config.clone();
                self.app_settings.remember_project(proj.root.clone());
                let _ = self.app_settings.save();
                self.project = Some(proj);
                self.error_msg.clear();
                self.script_dirty = false;
                self.preview_image = None;
                self.status_msg = format!("已打开 {}", path.display());
                self.tab = Tab::Workflow;
                self.refresh_caches(true);
            }
            Err(e) => self.error_msg = format!("{e:#}"),
        }
    }

    fn create_project(&mut self) {
        let name = self.new_name.trim();
        if name.is_empty() {
            self.error_msg = "请填写项目名称".into();
            return;
        }
        let path = self.create_parent.join(name);
        match Project::create(&path, name, &self.new_style, &self.new_aspect) {
            Ok(proj) => {
                self.edit_config = proj.config.clone();
                self.app_settings.remember_project(proj.root.clone());
                let _ = self.app_settings.save();
                self.project = Some(proj);
                self.error_msg.clear();
                self.script_dirty = false;
                self.status_msg = format!("已创建 {}", path.display());
                self.tab = Tab::Script;
                self.refresh_caches(true);
            }
            Err(e) => self.error_msg = format!("{e:#}"),
        }
    }

    fn reload_project(&mut self) {
        if let Some(root) = self.project.as_ref().map(|p| p.root.clone()) {
            let keep_tab = self.tab;
            self.open_project(&root);
            self.tab = keep_tab;
        }
    }

    fn submit(&mut self, job: Job) {
        if self.busy {
            self.error_msg = "已有任务在运行中".into();
            return;
        }
        let root = if matches!(job, Job::TestEndpoint { .. }) {
            self.project
                .as_ref()
                .map(|p| p.root.clone())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        } else {
            let Some(proj) = self.project.as_ref() else {
                self.error_msg = "尚未打开项目".into();
                return;
            };
            proj.root.clone()
        };
        self.app_settings.apply_to_env();
        self.busy_kind = job_to_kind(&job);
        match self.worker.submit(root, job) {
            Ok(()) => {
                self.busy = true;
                self.error_msg.clear();
            }
            Err(e) => {
                self.busy_kind = None;
                self.error_msg = e;
            }
        }
    }

    fn poll_worker(&mut self) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            match msg {
                WorkerMsg::Started(label) => {
                    self.busy = true;
                    self.busy_label = label.clone();
                    self.progress = None;
                    self.status_msg = format!("运行中：{label}");
                    self.push_log(format!("▶ {label}"));
                }
                WorkerMsg::Log(line) => self.push_log(line),
                WorkerMsg::Progress {
                    current,
                    total,
                    detail,
                } => {
                    self.progress = Some((current, total, detail.clone()));
                    self.status_msg = if total > 0 {
                        format!("进度 {current}/{total} · {detail}")
                    } else {
                        detail
                    };
                }
                WorkerMsg::Finished { ok, message } => {
                    self.busy = false;
                    self.busy_label.clear();
                    self.busy_kind = None;
                    self.progress = None;
                    if ok {
                        self.status_msg = message.clone();
                        self.push_log(format!("✓ {message}"));
                        self.error_msg.clear();
                    } else {
                        self.error_msg = message.clone();
                        self.push_log(format!("✗ {message}"));
                        self.status_msg = "任务失败".into();
                    }
                    if !message.contains("端点") {
                        self.reload_project();
                    }
                }
            }
        }
    }

    fn refresh_caches(&mut self, force: bool) {
        if !force && self.last_refresh.elapsed() < self.refresh_interval {
            return;
        }
        self.last_refresh = Instant::now();
        let Some(proj) = self.project.as_ref() else {
            self.breakdown_text.clear();
            self.asset_thumbs.clear();
            self.storyboard_thumbs.clear();
            self.video_files.clear();
            return;
        };
        if !self.script_dirty {
            match proj.find_script() {
                Ok(p) => {
                    self.script_path = Some(p.clone());
                    self.script_text =
                        fs::read_to_string(&p).unwrap_or_else(|e| format!("（读取失败：{e}）"));
                }
                Err(_) => {
                    self.script_path = None;
                    if self.script_text.is_empty() || self.script_text.starts_with('（') {
                        self.script_text = "（尚未导入剧本 — 可直接输入并保存）".into();
                    }
                }
            }
        }
        if proj.parsed_path().exists() {
            self.breakdown_text = fs::read_to_string(proj.parsed_path())
                .unwrap_or_else(|e| format!("（读取失败：{e}）"));
        } else {
            self.breakdown_text = "（请先运行「解析」生成 breakdown.json）".into();
        }
        self.asset_thumbs = collect_images(&proj.assets_dir());
        self.storyboard_thumbs = collect_images(&proj.storyboard_dir());
        self.video_files = collect_ext(&proj.video_dir(), &["mp4", "webm", "mov"]);
    }

    fn save_app_settings(&mut self) {
        self.app_settings.apply_to_env();
        match self.app_settings.save() {
            Ok(()) => {
                self.status_msg = format!("密钥已保存 → {}", AppSettings::config_path().display());
                self.settings_dirty = false;
            }
            Err(e) => self.error_msg = format!("{e:#}"),
        }
    }

    fn save_config(&mut self) {
        let Some(proj) = self.project.as_mut() else {
            return;
        };
        proj.config = self.edit_config.clone();
        match proj.save_config() {
            Ok(()) => {
                self.status_msg = "已保存 project.toml".into();
                self.settings_dirty = false;
            }
            Err(e) => self.error_msg = format!("{e:#}"),
        }
    }

    fn import_script_dialog(&mut self) {
        let Some(proj) = self.project.as_ref() else {
            return;
        };
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("剧本", &["md", "txt", "fountain"])
            .pick_file()
        {
            match stages::import::run(proj, &path) {
                Ok(()) => {
                    self.status_msg = format!("已导入 {}", path.display());
                    self.script_dirty = false;
                    self.refresh_caches(true);
                    self.tab = Tab::Script;
                }
                Err(e) => self.error_msg = format!("{e:#}"),
            }
        }
    }

    fn save_script(&mut self) {
        let Some(proj) = self.project.as_ref() else {
            return;
        };
        let path = if let Some(p) = self.script_path.clone() {
            p
        } else {
            let dir = proj.script_dir();
            if let Err(e) = fs::create_dir_all(&dir) {
                self.error_msg = format!("创建 script 目录失败: {e}");
                return;
            }
            dir.join("script.md")
        };
        match fs::write(&path, &self.script_text) {
            Ok(()) => {
                self.script_path = Some(path.clone());
                self.script_dirty = false;
                self.status_msg = format!("剧本已保存 → {}", path.display());
            }
            Err(e) => self.error_msg = format!("保存失败: {e}"),
        }
    }

    fn texture_for(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(path) {
            return Some(t.clone());
        }
        let bytes = fs::read(path).ok()?;
        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(path.to_string_lossy(), color, egui::TextureOptions::LINEAR);
        self.textures.insert(path.to_path_buf(), tex.clone());
        Some(tex)
    }

    fn handle_wf_action(&mut self, action: WfAction) {
        match action {
            WfAction::None | WfAction::Select(_) => {}
            WfAction::OpenTab(kind) => {
                self.tab = match kind {
                    WfNodeKind::Script => Tab::Script,
                    WfNodeKind::Parse => Tab::Parse,
                    WfNodeKind::Assets => Tab::Assets,
                    WfNodeKind::Storyboard => Tab::Storyboard,
                    WfNodeKind::Video | WfNodeKind::Export => Tab::Video,
                };
            }
            WfAction::Run(kind) => match kind {
                WfNodeKind::Script => self.import_script_dialog(),
                WfNodeKind::Parse => self.submit(Job::Parse {
                    dry_run: self.dry_run,
                }),
                WfNodeKind::Assets => self.submit(Job::Assets {
                    only: None,
                    dry_run: self.dry_run,
                }),
                WfNodeKind::Storyboard => self.submit(Job::Storyboard {
                    scene: None,
                    shot: None,
                    dry_run: self.dry_run,
                }),
                WfNodeKind::Video => self.submit(Job::Video {
                    shot: None,
                    concat: false,
                    dry_run: self.dry_run,
                }),
                WfNodeKind::Export => self.submit(Job::Video {
                    shot: None,
                    concat: true,
                    dry_run: self.dry_run,
                }),
            },
            WfAction::Approve(stage) => self.submit(Job::Approve { stage }),
        }
    }

    fn resolved_key_url_for_test(
        &self,
        family: ProviderKind,
    ) -> (String, String, String, EndpointMode) {
        match family {
            ProviderKind::OpenAi => {
                let mode = self.edit_config.openai_mode;
                let url = match mode {
                    EndpointMode::Official => OPENAI_OFFICIAL_BASE.to_string(),
                    EndpointMode::Custom => self.edit_config.openai_custom_base_url.clone(),
                };
                let key = match mode {
                    EndpointMode::Official => self.app_settings.openai_official_key.clone(),
                    EndpointMode::Custom => self.app_settings.openai_custom_key.clone(),
                };
                (
                    "OpenAI / Image2".into(),
                    url,
                    key,
                    mode,
                )
            }
            ProviderKind::Google => {
                let mode = self.edit_config.google_mode;
                let url = match mode {
                    EndpointMode::Official => GOOGLE_OFFICIAL_BASE.to_string(),
                    EndpointMode::Custom => self.edit_config.google_custom_base_url.clone(),
                };
                let key = match mode {
                    EndpointMode::Official => self.app_settings.google_official_key.clone(),
                    EndpointMode::Custom => self.app_settings.google_custom_key.clone(),
                };
                ("Google / Veo".into(), url, key, mode)
            }
            ProviderKind::Xai => {
                let mode = self.edit_config.xai_mode;
                let url = match mode {
                    EndpointMode::Official => XAI_OFFICIAL_BASE.to_string(),
                    EndpointMode::Custom => self.edit_config.xai_custom_base_url.clone(),
                };
                let key = match mode {
                    EndpointMode::Official => self.app_settings.xai_official_key.clone(),
                    EndpointMode::Custom => self.app_settings.xai_custom_key.clone(),
                };
                ("xAI / Grok".into(), url, key, mode)
            }
        }
    }
}

impl eframe::App for AdramaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        self.refresh_caches(false);
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        self.ui_top_bar(ctx);
        self.ui_status_bar(ctx);
        self.ui_side_rail(ctx);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Home => self.ui_home(ui),
                Tab::Workflow => self.ui_workflow(ui),
                Tab::Script => self.ui_script(ui),
                Tab::Parse => self.ui_parse(ui),
                Tab::Assets => self.ui_assets(ui, ctx),
                Tab::Storyboard => self.ui_storyboard(ui, ctx),
                Tab::Video => self.ui_video(ui),
                Tab::Settings => self.ui_settings(ui),
            });

        self.ui_preview_window(ctx);
    }
}

impl AdramaApp {
    fn ui_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .exact_height(52.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("my-adrama")
                            .strong()
                            .size(18.0)
                            .color(theme::ACCENT),
                    );
                    ui.label(RichText::new("AI 短剧工作流").color(theme::TEXT_MUTED));
                    ui.separator();

                    if ui.button("打开").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.open_project(&path);
                        }
                    }
                    if ui.button("新建").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.create_parent = path;
                        }
                        self.tab = Tab::Home;
                    }
                    if ui
                        .add_enabled(self.project.is_some(), egui::Button::new("刷新"))
                        .clicked()
                    {
                        self.reload_project();
                    }

                    if let Some(p) = self.project.as_ref() {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("📁 {}", p.config.name))
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new(p.root.display().to_string())
                                .small()
                                .color(theme::TEXT_DIM),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut self.dry_run, "演练 Dry-run");
                        if self.busy {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("取消").color(theme::DANGER),
                                    )
                                    .fill(Color32::from_rgb(50, 28, 32)),
                                )
                                .clicked()
                            {
                                self.worker.cancel();
                                self.push_log("… 正在请求取消");
                            }
                            ui.spinner();
                            ui.colored_label(theme::INFO, &self.busy_label);
                            if let Some((c, t, _)) = &self.progress {
                                if *t > 0 {
                                    ui.add(
                                        egui::ProgressBar::new(*c as f32 / *t as f32)
                                            .desired_width(100.0)
                                            .text(format!("{c}/{t}")),
                                    );
                                }
                            }
                        }
                    });
                });
            });
    }

    fn ui_status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bottom")
            .exact_height(if self.error_msg.is_empty() { 32.0 } else { 52.0 })
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ctx, |ui| {
                if !self.error_msg.is_empty() {
                    ui.colored_label(theme::DANGER, &self.error_msg);
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.status_msg).color(theme::TEXT_MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(p) = self.project.as_ref() {
                            ui.label(
                                RichText::new(format!(
                                    "对话 {} · 图像 {} · 视频 {}",
                                    p.config.chat_provider.label_zh(),
                                    p.config.image_provider.label_zh(),
                                    p.config.video_provider.label_zh(),
                                ))
                                .small()
                                .color(theme::TEXT_DIM),
                            );
                        }
                    });
                });
            });
    }

    fn ui_side_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("rail")
            .exact_width(200.0)
            .resizable(false)
            .frame(theme::rail_frame())
            .show(ctx, |ui| {
                ui.label(RichText::new("导航").small().color(theme::TEXT_DIM));
                ui.add_space(4.0);
                self.nav_item(ui, Tab::Home, "⌂", "首页");
                self.nav_item(ui, Tab::Workflow, "◈", "工作流");
                ui.add_space(8.0);
                ui.label(RichText::new("生产阶段").small().color(theme::TEXT_DIM));
                self.nav_item(ui, Tab::Script, "1", "剧本");
                self.nav_item(ui, Tab::Parse, "2", "解析");
                self.nav_item(ui, Tab::Assets, "3", "资产");
                self.nav_item(ui, Tab::Storyboard, "4", "分镜");
                self.nav_item(ui, Tab::Video, "5", "视频");
                ui.add_space(8.0);
                self.nav_item(ui, Tab::Settings, "⚙", "设置");

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(RichText::new("流水线").small().color(theme::TEXT_DIM));
                if let Some(proj) = self.project.as_ref() {
                    for stage in Stage::all() {
                        let st = proj.state.get(*stage);
                        let color = match st {
                            StageStatus::Pending => theme::TEXT_DIM,
                            StageStatus::InProgress => theme::INFO,
                            StageStatus::Done => theme::SUCCESS,
                            StageStatus::Approved => theme::WARNING,
                        };
                        ui.horizontal(|ui| {
                            ui.colored_label(color, status_dot(st));
                            ui.label(stage_zh(*stage));
                            ui.label(RichText::new(status_zh(st)).small().color(theme::TEXT_DIM));
                        });
                    }
                } else {
                    ui.label(RichText::new("未打开项目").color(theme::TEXT_DIM));
                }

                ui.add_space(10.0);
                if self.project.is_some() {
                    if ui.button("打开项目目录").clicked() {
                        if let Some(p) = self.project.as_ref() {
                            open_path(&p.root);
                        }
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("日志").small().color(theme::TEXT_DIM));
                        if ui.small_button("清空").clicked() {
                            self.logs.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in self.logs.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
                                ui.label(
                                    RichText::new(line)
                                        .small()
                                        .monospace()
                                        .color(theme::TEXT_MUTED),
                                );
                            }
                        });
                });
            });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, tab: Tab, icon: &str, label: &str) {
        let selected = self.tab == tab;
        let fill = if selected {
            theme::ACCENT_SOFT
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if selected {
            egui::Stroke::new(1.0_f32, theme::ACCENT)
        } else {
            egui::Stroke::NONE
        };
        let resp = egui::Frame::new()
            .fill(fill)
            .stroke(stroke)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).color(if selected {
                        theme::ACCENT
                    } else {
                        theme::TEXT_MUTED
                    }));
                    ui.label(RichText::new(label).color(if selected {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    }));
                });
            })
            .response
            .interact(Sense::click());
        if resp.hovered() && !selected {
            ui.painter().rect_filled(
                resp.rect,
                8.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 8),
            );
        }
        if resp.clicked() {
            self.tab = tab;
        }
    }

    fn ui_home(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("首页").color(theme::TEXT));
        ui.label(
            RichText::new("剧本 → 解析 → 资产 → 分镜 → 视频 → 成片")
                .color(theme::TEXT_MUTED),
        );
        ui.add_space(12.0);

        ui.columns(2, |cols| {
            theme::card_frame().show(&mut cols[0], |ui| {
                ui.label(RichText::new("快速开始").strong().size(16.0));
                ui.add_space(8.0);
                if !self.app_settings.recent_projects.is_empty() {
                    ui.label(RichText::new("最近项目").color(theme::TEXT_MUTED));
                    let recent = self.app_settings.recent_projects.clone();
                    for p in recent.into_iter().take(6) {
                        ui.horizontal(|ui| {
                            let name = p
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("project");
                            if ui.link(name).on_hover_text(p.display().to_string()).clicked() {
                                self.open_project(&p);
                            }
                        });
                    }
                    ui.add_space(8.0);
                }
                ui.horizontal(|ui| {
                    if ui.button("打开项目文件夹…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.open_project(&path);
                        }
                    }
                    if ui.button("配置 API 密钥").clicked() {
                        self.tab = Tab::Settings;
                        self.settings_pane = SettingsPane::Keys;
                    }
                });
            });

            theme::card_frame().show(&mut cols[1], |ui| {
                ui.label(RichText::new("新建项目").strong().size(16.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("目录");
                    ui.label(
                        RichText::new(self.create_parent.display().to_string())
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    if ui.small_button("浏览").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.create_parent = p;
                        }
                    }
                });
                labeled_field(ui, "名称", &mut self.new_name, 200.0);
                labeled_field(ui, "风格", &mut self.new_style, 280.0);
                ui.horizontal(|ui| {
                    ui.label("画幅");
                    for a in ["16:9", "9:16", "1:1"] {
                        ui.selectable_value(&mut self.new_aspect, a.to_string(), a);
                    }
                });
                if ui
                    .add(egui::Button::new("创建项目").fill(theme::ACCENT_SOFT))
                    .clicked()
                {
                    self.create_project();
                }
            });
        });

        if self.project.is_some() {
            ui.add_space(14.0);
            theme::card_frame().show(ui, |ui| {
                ui.label(RichText::new("当前项目").strong().size(16.0));
                if let Some(p) = self.project.as_ref() {
                    ui.label(format!("{} · {}", p.config.name, p.root.display()));
                    ui.label(format!("风格：{}", p.config.style));
                    ui.label(format!(
                        "产物：{} 资产 · {} 分镜 · {} 视频",
                        self.asset_thumbs.len(),
                        self.storyboard_thumbs.len(),
                        self.video_files.len()
                    ));
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    let en = !self.busy;
                    if ui.add_enabled(en, egui::Button::new("工作流画布")).clicked() {
                        self.tab = Tab::Workflow;
                    }
                    if ui.add_enabled(en, egui::Button::new("导入剧本")).clicked() {
                        self.import_script_dialog();
                    }
                    if ui.add_enabled(en, egui::Button::new("运行解析")).clicked() {
                        self.submit(Job::Parse {
                            dry_run: self.dry_run,
                        });
                    }
                    if ui.add_enabled(en, egui::Button::new("生成资产")).clicked() {
                        self.submit(Job::Assets {
                            only: None,
                            dry_run: self.dry_run,
                        });
                    }
                    if ui.add_enabled(en, egui::Button::new("生成分镜")).clicked() {
                        self.submit(Job::Storyboard {
                            scene: None,
                            shot: None,
                            dry_run: self.dry_run,
                        });
                    }
                    if ui.add_enabled(en, egui::Button::new("生成视频")).clicked() {
                        self.submit(Job::Video {
                            shot: None,
                            concat: self.video_concat,
                            dry_run: self.dry_run,
                        });
                    }
                });
            });
        }
    }

    fn ui_workflow(&mut self, ui: &mut egui::Ui) {
        if self.project.is_none() {
            theme::card_frame().show(ui, |ui| {
                ui.heading("工作流");
                ui.label("请先在首页打开或新建项目。");
                if ui.button("返回首页").clicked() {
                    self.tab = Tab::Home;
                }
            });
            return;
        }
        let action = self
            .workflow
            .show(ui, self.project.as_ref(), self.busy, self.busy_kind);
        self.handle_wf_action(action);
    }

    fn ui_script(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "剧本", "导入、编辑并保存剧本文本");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("导入…"))
                .clicked()
            {
                self.import_script_dialog();
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new(if self.script_dirty { "保存 *" } else { "保存" }),
                )
                .clicked()
            {
                self.save_script();
            }
            if let Some(p) = &self.script_path {
                ui.label(RichText::new(p.display().to_string()).small().color(theme::TEXT_DIM));
            }
        });
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.script_text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(30)
                        .font(egui::TextStyle::Monospace),
                );
                if r.changed() {
                    self.script_dirty = true;
                }
            });
        });
    }

    fn ui_parse(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "阶段 · 解析", "LLM 将剧本转为角色 / 场景 / 镜头结构");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("运行解析"))
                .clicked()
            {
                self.submit(Job::Parse {
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("审核通过"))
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Parse,
                });
            }
        });
        ui.add_space(8.0);
        if let Some(proj) = self.project.as_ref() {
            if let Ok(bd) = proj.load_breakdown() {
                theme::section_frame().show(ui, |ui| {
                    self.ui_breakdown_summary(ui, &bd);
                });
                ui.add_space(8.0);
            }
        }
        theme::card_frame().show(ui, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.breakdown_text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(24)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            });
        });
    }

    fn ui_breakdown_summary(&self, ui: &mut egui::Ui, bd: &Breakdown) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&bd.title).strong());
            if !bd.summary.is_empty() {
                ui.label(RichText::new(&bd.summary).italics().color(theme::TEXT_MUTED));
            }
        });
        ui.collapsing(format!("角色（{}）", bd.characters.len()), |ui| {
            for c in &bd.characters {
                ui.label(format!("• {} — {}", c.name, truncate(&c.appearance, 80)));
            }
        });
        ui.collapsing(format!("镜头（{}）", bd.shots.len()), |ui| {
            for s in &bd.shots {
                ui.label(format!(
                    "• {}  [{}] {} — {}",
                    s.id,
                    s.framing,
                    s.camera,
                    truncate(&s.visual, 70)
                ));
            }
        });
    }

    fn ui_assets(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.page_header(ui, "阶段 · 资产", "角色定妆照、服装、道具、场景参考图");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("全部生成"))
                .clicked()
            {
                self.submit(Job::Assets {
                    only: None,
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("审核通过"))
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Assets,
                });
            }
            ui.label("仅生成");
            ui.add(egui::TextEdit::singleline(&mut self.only_asset).desired_width(120.0));
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some() && !self.only_asset.trim().is_empty(),
                    egui::Button::new("生成"),
                )
                .clicked()
            {
                self.submit(Job::Assets {
                    only: Some(self.only_asset.trim().to_string()),
                    dry_run: self.dry_run,
                });
            }
        });
        self.ui_redo_row(ui, Stage::Assets);
        ui.add_space(8.0);
        self.ui_image_grid(ui, ctx, &self.asset_thumbs.clone());
    }

    fn ui_storyboard(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.page_header(ui, "阶段 · 分镜", "按镜头生成参考图，保持角色一致性");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("全部生成"))
                .clicked()
            {
                self.submit(Job::Storyboard {
                    scene: None,
                    shot: None,
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("审核通过"))
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Storyboard,
                });
            }
            ui.label("场次");
            ui.add(egui::TextEdit::singleline(&mut self.scene_filter).desired_width(48.0));
            ui.label("镜头");
            ui.add(egui::TextEdit::singleline(&mut self.shot_filter).desired_width(100.0));
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("筛选生成"))
                .clicked()
            {
                self.submit(Job::Storyboard {
                    scene: self.scene_filter.trim().parse().ok(),
                    shot: nonempty(&self.shot_filter),
                    dry_run: self.dry_run,
                });
            }
        });
        self.ui_redo_row(ui, Stage::Storyboard);
        ui.add_space(8.0);
        self.ui_image_grid(ui, ctx, &self.storyboard_thumbs.clone());
    }

    fn ui_video(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "阶段 · 视频", "分镜图生视频，可选 ffmpeg 拼接成片");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("全部生成"))
                .clicked()
            {
                self.submit(Job::Video {
                    shot: None,
                    concat: self.video_concat,
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("审核通过"))
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Video,
                });
            }
            ui.checkbox(&mut self.video_concat, "生成后拼接");
            ui.label("镜头");
            ui.add(egui::TextEdit::singleline(&mut self.shot_filter).desired_width(100.0));
            if ui
                .add_enabled(!self.busy && self.project.is_some(), egui::Button::new("生成镜头"))
                .clicked()
            {
                self.submit(Job::Video {
                    shot: nonempty(&self.shot_filter),
                    concat: self.video_concat,
                    dry_run: self.dry_run,
                });
            }
        });
        self.ui_redo_row(ui, Stage::Video);
        ui.add_space(8.0);

        if let Some(proj) = self.project.as_ref() {
            let final_mp4 = proj.video_dir().join("final.mp4");
            if final_mp4.exists() {
                theme::section_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(theme::SUCCESS, "成片 final.mp4 已就绪");
                        if ui.button("播放").clicked() {
                            open_path(&final_mp4);
                        }
                    });
                });
                ui.add_space(8.0);
            }
        }

        theme::card_frame().show(ui, |ui| {
            ui.label(RichText::new("视频片段").strong());
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.video_files.is_empty() {
                    ui.label(RichText::new("（暂无视频）").color(theme::TEXT_DIM));
                }
                for path in self.video_files.clone() {
                    ui.horizontal(|ui| {
                        ui.monospace(
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("?"),
                        );
                        if ui.small_button("打开").clicked() {
                            open_path(&path);
                        }
                        if ui.small_button("文件夹").clicked() {
                            if let Some(parent) = path.parent() {
                                open_path(parent);
                            }
                        }
                    });
                }
            });
        });
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        self.page_header(
            ui,
            "设置",
            "各厂商支持「官方 / 自定义」独立密钥与端点",
        );

        ui.horizontal(|ui| {
            for (pane, label) in [
                (SettingsPane::Keys, "API 密钥"),
                (SettingsPane::Routing, "能力路由"),
                (SettingsPane::Project, "项目配置"),
            ] {
                if ui
                    .selectable_label(self.settings_pane == pane, label)
                    .clicked()
                {
                    self.settings_pane = pane;
                }
            }
        });
        ui.add_space(10.0);

        egui::ScrollArea::vertical().show(ui, |ui| match self.settings_pane {
            SettingsPane::Keys => self.ui_settings_keys(ui),
            SettingsPane::Routing => self.ui_settings_routing(ui),
            SettingsPane::Project => self.ui_settings_project(ui),
        });
    }

    fn ui_settings_keys(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(format!(
                "密钥文件：{}",
                AppSettings::config_path().display()
            ))
            .small()
            .color(theme::TEXT_DIM),
        );
        ui.add_space(8.0);

        let mut dirty = self.settings_dirty;
        let mut test_openai = false;
        let mut test_google = false;
        let mut test_xai = false;

        theme::card_frame().show(ui, |ui| {
            vendor_key_card_ui(
                ui,
                "OpenAI / Image2",
                "对话解析 · 图像生成（gpt-image）",
                theme::STAGE_ASSETS,
                &mut self.edit_config.openai_mode,
                &mut self.app_settings.openai_official_key,
                &mut self.app_settings.openai_custom_key,
                OPENAI_OFFICIAL_BASE,
                &mut self.edit_config.openai_custom_base_url,
                !self.busy,
                &mut dirty,
                &mut test_openai,
            );
        });
        ui.add_space(10.0);
        theme::card_frame().show(ui, |ui| {
            vendor_key_card_ui(
                ui,
                "Google / Gemini / Veo",
                "视频生成（Veo / Omni）· 可选对话/图像",
                theme::STAGE_VIDEO,
                &mut self.edit_config.google_mode,
                &mut self.app_settings.google_official_key,
                &mut self.app_settings.google_custom_key,
                GOOGLE_OFFICIAL_BASE,
                &mut self.edit_config.google_custom_base_url,
                !self.busy,
                &mut dirty,
                &mut test_google,
            );
        });
        ui.add_space(10.0);
        theme::card_frame().show(ui, |ui| {
            vendor_key_card_ui(
                ui,
                "xAI / Grok",
                "Grok 对话 · 图像 · 视频",
                theme::STAGE_STORY,
                &mut self.edit_config.xai_mode,
                &mut self.app_settings.xai_official_key,
                &mut self.app_settings.xai_custom_key,
                XAI_OFFICIAL_BASE,
                &mut self.edit_config.xai_custom_base_url,
                !self.busy,
                &mut dirty,
                &mut test_xai,
            );
        });
        self.settings_dirty = dirty;

        if test_openai {
            self.run_connection_test(ProviderKind::OpenAi);
        }
        if test_google {
            self.run_connection_test(ProviderKind::Google);
        }
        if test_xai {
            self.run_connection_test(ProviderKind::Xai);
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new("保存全部密钥").fill(theme::ACCENT_SOFT))
                .clicked()
            {
                self.save_app_settings();
                if self.project.is_some() {
                    self.save_config();
                }
            }
            if ui.button("仅应用密钥到会话").clicked() {
                self.app_settings.apply_to_env();
                self.status_msg = "密钥已应用到当前会话".into();
            }
        });
    }

    fn run_connection_test(&mut self, family: ProviderKind) {
        self.app_settings.apply_to_env();
        let (kind, base_url, api_key, mode) = self.resolved_key_url_for_test(family);
        let model = match family {
            ProviderKind::OpenAi => self.edit_config.openai_chat_model.clone(),
            ProviderKind::Google => self.edit_config.google_video_model.clone(),
            ProviderKind::Xai => self.edit_config.xai_chat_model.clone(),
        };
        self.submit(Job::TestEndpoint {
            kind: format!("{kind} · {}", mode.label_zh()),
            base_url,
            api_key,
            model,
        });
    }

    fn ui_settings_routing(&mut self, ui: &mut egui::Ui) {
        if self.project.is_none() {
            ui.label("打开项目后可配置能力路由与模型。");
            return;
        }
        theme::card_frame().show(ui, |ui| {
            ui.label(RichText::new("能力使用哪家服务").strong().size(15.0));
            ui.label(
                RichText::new("每家服务在「API 密钥」页各自选择官方或自定义。")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(10.0);
            provider_pick(ui, "对话 / 剧本解析", &mut self.edit_config.chat_provider, &mut self.settings_dirty);
            provider_pick(ui, "图像（资产 / 分镜）", &mut self.edit_config.image_provider, &mut self.settings_dirty);
            provider_pick(ui, "视频（图生视频）", &mut self.edit_config.video_provider, &mut self.settings_dirty);

            ui.add_space(12.0);
            ui.separator();
            ui.label(RichText::new("模型 ID").strong());
            ui.add_space(6.0);

            ui.collapsing("OpenAI / Image2 模型", |ui| {
                field(ui, "对话模型", &mut self.edit_config.openai_chat_model, 260.0, &mut self.settings_dirty);
                field(ui, "图像模型", &mut self.edit_config.openai_image_model, 260.0, &mut self.settings_dirty);
            });
            ui.collapsing("Google 模型", |ui| {
                field(ui, "对话模型", &mut self.edit_config.google_chat_model, 260.0, &mut self.settings_dirty);
                field(ui, "图像模型", &mut self.edit_config.google_image_model, 260.0, &mut self.settings_dirty);
                field(ui, "视频模型", &mut self.edit_config.google_video_model, 280.0, &mut self.settings_dirty);
            });
            ui.collapsing("xAI / Grok 模型", |ui| {
                field(ui, "对话模型", &mut self.edit_config.xai_chat_model, 260.0, &mut self.settings_dirty);
                field(ui, "图像模型", &mut self.edit_config.xai_image_model, 260.0, &mut self.settings_dirty);
                field(ui, "视频模型", &mut self.edit_config.xai_video_model, 260.0, &mut self.settings_dirty);
                field(
                    ui,
                    "视频自定义 URL（可空）",
                    &mut self.edit_config.xai_video_base_url,
                    320.0,
                    &mut self.settings_dirty,
                );
            });

            ui.add_space(10.0);
            if ui
                .add(egui::Button::new("保存路由与模型").fill(theme::ACCENT_SOFT))
                .clicked()
            {
                self.save_config();
            }
        });
    }

    fn ui_settings_project(&mut self, ui: &mut egui::Ui) {
        if self.project.is_none() {
            ui.label("打开项目后可编辑 project.toml。");
            return;
        }
        theme::card_frame().show(ui, |ui| {
            ui.label(RichText::new("项目基本信息").strong().size(15.0));
            field(ui, "名称", &mut self.edit_config.name, 240.0, &mut self.settings_dirty);
            field(ui, "风格前缀", &mut self.edit_config.style, 400.0, &mut self.settings_dirty);
            ui.horizontal(|ui| {
                ui.label("画幅");
                for a in ["16:9", "9:16", "1:1"] {
                    if ui
                        .selectable_value(&mut self.edit_config.aspect, a.to_string(), a)
                        .changed()
                    {
                        self.settings_dirty = true;
                    }
                }
            });
            ui.add_space(10.0);
            if ui
                .add(egui::Button::new(if self.settings_dirty {
                    "保存 project.toml *"
                } else {
                    "保存 project.toml"
                })
                .fill(theme::ACCENT_SOFT))
                .clicked()
            {
                self.save_config();
            }
            if ui.button("从磁盘重新加载").clicked() {
                self.reload_project();
            }
        });
    }

    fn page_header(&self, ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new(title).color(theme::TEXT));
        });
        ui.label(RichText::new(subtitle).color(theme::TEXT_MUTED));
        ui.add_space(8.0);
    }

    fn ui_redo_row(&mut self, ui: &mut egui::Ui, stage: Stage) {
        ui.horizontal(|ui| {
            ui.label("重生成 ID");
            ui.add(egui::TextEdit::singleline(&mut self.redo_id).desired_width(140.0));
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some() && !self.redo_id.trim().is_empty(),
                    egui::Button::new("重生成"),
                )
                .clicked()
            {
                self.submit(Job::Redo {
                    stage,
                    id: self.redo_id.trim().to_string(),
                    dry_run: self.dry_run,
                });
            }
        });
    }

    fn ui_image_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, paths: &[PathBuf]) {
        if paths.is_empty() {
            theme::card_frame().show(ui, |ui| {
                ui.label(RichText::new("（暂无图片）").color(theme::TEXT_DIM));
            });
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            let thumb = 156.0;
            ui.horizontal_wrapped(|ui| {
                for path in paths {
                    theme::section_frame().show(ui, |ui| {
                        ui.set_width(thumb + 8.0);
                        if let Some(tex) = self.texture_for(ctx, path) {
                            let size = tex.size_vec2();
                            let scale = (thumb / size.x).min(thumb / size.y).min(1.0);
                            let img = egui::Image::new((tex.id(), size * scale))
                                .maintain_aspect_ratio(true)
                                .corner_radius(6.0);
                            if ui
                                .add(img)
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                self.preview_image = Some(path.clone());
                            }
                        }
                        let name = path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("?");
                        ui.label(RichText::new(name).small().color(theme::TEXT_MUTED));
                        ui.horizontal(|ui| {
                            if ui.small_button("预览").clicked() {
                                self.preview_image = Some(path.clone());
                            }
                            if ui.small_button("打开").clicked() {
                                open_path(path);
                            }
                        });
                        if let Some(st) = sibling_item_status(path) {
                            ui.label(
                                RichText::new(item_status_zh(st))
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                        }
                    });
                }
            });
        });
    }

    fn ui_preview_window(&mut self, ctx: &egui::Context) {
        if self.preview_image.is_none() {
            return;
        }
        let mut open = true;
        egui::Window::new("图片预览")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([760.0, 560.0])
            .frame(theme::card_frame())
            .show(ctx, |ui| {
                if let Some(path) = self.preview_image.clone() {
                    ui.label(RichText::new(path.display().to_string()).small());
                    ui.separator();
                    if let Some(tex) = self.texture_for(ctx, &path) {
                        let avail = ui.available_size() - Vec2::new(0.0, 40.0);
                        let size = tex.size_vec2();
                        let scale = (avail.x / size.x).min(avail.y / size.y).min(1.0).max(0.05);
                        ui.image((tex.id(), size * scale));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("系统打开").clicked() {
                            open_path(&path);
                        }
                        if ui.button("关闭").clicked() {
                            self.preview_image = None;
                        }
                    });
                }
            });
        if !open {
            self.preview_image = None;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn vendor_key_card_ui(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    accent: Color32,
    mode: &mut EndpointMode,
    official_key: &mut String,
    custom_key: &mut String,
    official_url: &str,
    custom_url: &mut String,
    can_test: bool,
    dirty: &mut bool,
    test_clicked: &mut bool,
) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 5.0, accent);
        ui.label(RichText::new(title).strong().size(15.0));
    });
    ui.label(RichText::new(subtitle).small().color(theme::TEXT_MUTED));
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("端点模式");
        for m in [EndpointMode::Official, EndpointMode::Custom] {
            if ui.selectable_value(mode, m, m.label_zh()).changed() {
                *dirty = true;
            }
        }
    });

    ui.add_space(6.0);
    match *mode {
        EndpointMode::Official => {
            ui.label(
                RichText::new(format!("官方地址：{official_url}"))
                    .small()
                    .monospace()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                ui.set_min_width(100.0);
                ui.label("官方密钥");
                if ui
                    .add(
                        egui::TextEdit::singleline(official_key)
                            .password(true)
                            .desired_width(360.0)
                            .hint_text("sk-... / AIza..."),
                    )
                    .changed()
                {
                    *dirty = true;
                }
            });
        }
        EndpointMode::Custom => {
            ui.horizontal(|ui| {
                ui.set_min_width(100.0);
                ui.label("自定义 URL");
                if ui
                    .add(
                        egui::TextEdit::singleline(custom_url)
                            .desired_width(360.0)
                            .hint_text("https://proxy.example.com/v1"),
                    )
                    .changed()
                {
                    *dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.set_min_width(100.0);
                ui.label("自定义密钥");
                if ui
                    .add(
                        egui::TextEdit::singleline(custom_key)
                            .password(true)
                            .desired_width(360.0),
                    )
                    .changed()
                {
                    *dirty = true;
                }
            });
        }
    }

    ui.add_space(6.0);
    if ui
        .add_enabled(can_test, egui::Button::new("测试当前模式连接"))
        .clicked()
    {
        *test_clicked = true;
    }
}

fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32) {
    ui.horizontal(|ui| {
        ui.set_min_width(48.0);
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(width));
    });
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32, dirty: &mut bool) {
    ui.horizontal(|ui| {
        ui.set_min_width(120.0);
        ui.label(label);
        if ui
            .add(egui::TextEdit::singleline(value).desired_width(width))
            .changed()
        {
            *dirty = true;
        }
    });
}

fn provider_pick(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut ProviderKind,
    dirty: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.set_min_width(140.0);
        ui.label(label);
        for p in [
            ProviderKind::OpenAi,
            ProviderKind::Google,
            ProviderKind::Xai,
        ] {
            if ui.selectable_value(value, p, p.label_zh()).changed() {
                *dirty = true;
            }
        }
    });
}

fn job_to_kind(job: &Job) -> Option<WfNodeKind> {
    match job {
        Job::Parse { .. } => Some(WfNodeKind::Parse),
        Job::Assets { .. } => Some(WfNodeKind::Assets),
        Job::Storyboard { .. } => Some(WfNodeKind::Storyboard),
        Job::Video { concat, .. } => {
            if *concat {
                Some(WfNodeKind::Export)
            } else {
                Some(WfNodeKind::Video)
            }
        }
        Job::Redo { stage, .. } => match stage {
            Stage::Parse => Some(WfNodeKind::Parse),
            Stage::Assets => Some(WfNodeKind::Assets),
            Stage::Storyboard => Some(WfNodeKind::Storyboard),
            Stage::Video => Some(WfNodeKind::Video),
        },
        Job::Approve { .. } | Job::TestEndpoint { .. } => None,
    }
}

fn stage_zh(s: Stage) -> &'static str {
    match s {
        Stage::Parse => "解析",
        Stage::Assets => "资产",
        Stage::Storyboard => "分镜",
        Stage::Video => "视频",
    }
}

fn status_zh(st: StageStatus) -> &'static str {
    match st {
        StageStatus::Pending => "待处理",
        StageStatus::InProgress => "进行中",
        StageStatus::Done => "已完成",
        StageStatus::Approved => "已审核",
    }
}

fn status_dot(st: StageStatus) -> &'static str {
    match st {
        StageStatus::Pending => "○",
        StageStatus::InProgress => "◐",
        StageStatus::Done => "●",
        StageStatus::Approved => "★",
    }
}

fn item_status_zh(st: ItemStatus) -> &'static str {
    match st {
        ItemStatus::Pending => "待处理",
        ItemStatus::Generating => "生成中",
        ItemStatus::Done => "完成",
        ItemStatus::Failed => "失败",
        ItemStatus::Approved => "已审核",
    }
}

fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn collect_images(root: &Path) -> Vec<PathBuf> {
    collect_ext(root, &["png", "jpg", "jpeg", "webp"])
}

fn collect_ext(root: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, exts, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    walk(root, exts, &mut out);
    out.sort();
    out
}

fn sibling_item_status(path: &Path) -> Option<ItemStatus> {
    let stem = path.file_stem()?.to_str()?;
    let parent = path.parent()?;
    let candidates = [
        parent.join("meta.json"),
        parent.join(format!("{stem}.json")),
        parent.join(format!(
            "{}.json",
            stem.trim_end_matches("_front")
                .trim_end_matches("_side")
                .trim_end_matches("_full")
        )),
    ];
    for c in candidates {
        if !c.exists() {
            continue;
        }
        if let Ok(s) = fs::read_to_string(&c) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(st) = v.get("status").and_then(|x| x.as_str()) {
                    return match st {
                        "pending" => Some(ItemStatus::Pending),
                        "generating" => Some(ItemStatus::Generating),
                        "done" => Some(ItemStatus::Done),
                        "failed" => Some(ItemStatus::Failed),
                        "approved" => Some(ItemStatus::Approved),
                        _ => None,
                    };
                }
            }
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.display().to_string()])
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
}
