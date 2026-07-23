use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText, TextureHandle, Vec2};

use super::worker::{Job, WorkerHandle, WorkerMsg};
use super::workflow::{WfAction, WfNodeKind, WorkflowCanvas};
use crate::model::{Breakdown, ItemStatus};
use crate::project::{Project, ProjectConfig, ProviderKind, Stage, StageStatus};
use crate::settings::AppSettings;
use crate::stages;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Workflow,
    Overview,
    Script,
    Parse,
    Assets,
    Storyboard,
    Video,
    Settings,
}

pub struct AdramaApp {
    worker: WorkerHandle,
    project: Option<Project>,
    tab: Tab,
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
    pub fn new(cc: &eframe::CreationContext<'_>, initial: PathBuf) -> Self {
        // dark-ish professional theme
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        cc.egui_ctx.set_style(style);

        let mut app_settings = AppSettings::load();
        app_settings.merge_from_env();
        app_settings.apply_to_env();

        let mut app = Self {
            worker: WorkerHandle::spawn(),
            project: None,
            tab: Tab::Workflow,
            busy: false,
            busy_label: String::new(),
            busy_kind: None,
            progress: None,
            logs: Vec::new(),
            status_msg: "打开或新建项目以开始。".into(),
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
            if last.join("project.toml").exists() {
                Some(last)
            } else {
                None
            }
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
            self.open_project(&root);
        }
    }

    fn submit(&mut self, job: Job) {
        if self.busy {
            self.error_msg = "已有任务在运行中".into();
            return;
        }
        // Connection tests may run without a project; use cwd as dummy root
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
                    // Avoid wiping in-progress script edits on test-endpoint finish
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
            self.script_text.clear();
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
                        self.script_text = "（尚未导入剧本 — 可直接在此输入并保存）".into();
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
                self.status_msg = format!(
                    "已保存全局设置 → {}",
                    AppSettings::config_path().display()
                );
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
                    self.refresh_caches(true);
                    self.tab = Tab::Script;
                }
                Err(e) => self.error_msg = format!("{e:#}"),
            }
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
        let tex = ctx.load_texture(
            path.to_string_lossy(),
            color,
            egui::TextureOptions::LINEAR,
        );
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
}

impl eframe::App for AdramaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        self.refresh_caches(false);
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("adrama").strong());
                ui.label(RichText::new("AI 短剧工作流").weak());
                ui.separator();
                if ui.button("打开项目…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.open_project(&path);
                    }
                }
                if ui.button("新建项目").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.create_parent = path;
                    }
                    self.tab = Tab::Overview;
                }
                if ui
                    .add_enabled(self.project.is_some(), egui::Button::new("重新加载"))
                    .clicked()
                {
                    self.reload_project();
                }
                ui.separator();
                if let Some(p) = self.project.as_ref() {
                    ui.label(
                        RichText::new(format!("{}  ·  {}", p.config.name, p.root.display()))
                            .strong(),
                    );
                } else {
                    ui.label(RichText::new("未打开项目").weak());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.busy {
                        if ui
                            .button(RichText::new("取消任务").color(Color32::from_rgb(255, 120, 120)))
                            .clicked()
                        {
                            self.worker.cancel();
                            self.push_log("… 正在请求取消");
                        }
                        ui.spinner();
                        ui.colored_label(Color32::LIGHT_BLUE, &self.busy_label);
                        if let Some((c, t, _)) = &self.progress {
                            if *t > 0 {
                                ui.add(
                                    egui::ProgressBar::new(*c as f32 / *t as f32)
                                        .desired_width(120.0)
                                        .text(format!("{c}/{t}")),
                                );
                            }
                        }
                    }
                    ui.checkbox(&mut self.dry_run, "演练模式 Dry-run");
                });
            });
            ui.add_space(2.0);
        });

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            if !self.error_msg.is_empty() {
                ui.colored_label(Color32::from_rgb(230, 90, 90), &self.error_msg);
            }
            ui.horizontal(|ui| {
                ui.label(&self.status_msg);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("设置可配置 Image2 / Omni·Veo / Grok 的 URL 与 Key")
                            .small()
                            .weak(),
                    );
                });
            });
        });

        egui::SidePanel::left("nav")
            .resizable(true)
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.heading("导航");
                ui.separator();
                self.nav_button(ui, Tab::Workflow, "◈  工作流画布");
                self.nav_button(ui, Tab::Overview, "⌂  项目总览");
                self.nav_button(ui, Tab::Script, "✎  剧本");
                self.nav_button(ui, Tab::Parse, "1  解析");
                self.nav_button(ui, Tab::Assets, "2  资产");
                self.nav_button(ui, Tab::Storyboard, "3  分镜");
                self.nav_button(ui, Tab::Video, "4  视频");
                self.nav_button(ui, Tab::Settings, "⚙  设置");

                ui.add_space(10.0);
                ui.separator();
                ui.label(RichText::new("流水线状态").strong());
                if let Some(proj) = self.project.as_ref() {
                    for stage in Stage::all() {
                        let st = proj.state.get(*stage);
                        let (mark, color) = match st {
                            StageStatus::Pending => ("○", Color32::GRAY),
                            StageStatus::InProgress => ("…", Color32::LIGHT_BLUE),
                            StageStatus::Done => ("●", Color32::from_rgb(100, 200, 100)),
                            StageStatus::Approved => ("★", Color32::from_rgb(240, 200, 40)),
                        };
                        ui.horizontal(|ui| {
                            ui.colored_label(color, mark);
                            ui.label(stage_zh(*stage));
                            ui.label(
                                RichText::new(status_zh(st)).small().weak(),
                            );
                        });
                    }
                } else {
                    ui.label(RichText::new("—").weak());
                }

                ui.add_space(10.0);
                ui.separator();
                if let Some(proj) = self.project.as_ref() {
                    if ui.button("打开项目文件夹").clicked() {
                        open_path(&proj.root);
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("运行日志").strong());
                    if ui.small_button("清空").clicked() {
                        self.logs.clear();
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.label(RichText::new(line).small().monospace());
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Workflow => self.ui_workflow(ui),
            Tab::Overview => self.ui_overview(ui),
            Tab::Script => self.ui_script(ui),
            Tab::Parse => self.ui_parse(ui),
            Tab::Assets => self.ui_assets(ui, ctx),
            Tab::Storyboard => self.ui_storyboard(ui, ctx),
            Tab::Video => self.ui_video(ui),
            Tab::Settings => self.ui_settings(ui),
        });

        // Lightbox preview
        if self.preview_image.is_some() {
            let mut open = true;
            egui::Window::new("图片预览")
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([720.0, 520.0])
                .show(ctx, |ui| {
                    if let Some(path) = self.preview_image.clone() {
                        ui.label(path.display().to_string());
                        ui.separator();
                        if let Some(tex) = self.texture_for(ctx, &path) {
                            let avail = ui.available_size();
                            let size = tex.size_vec2();
                            let scale = (avail.x / size.x)
                                .min(avail.y / size.y)
                                .min(1.0)
                                .max(0.1);
                            ui.image((tex.id(), size * scale));
                        }
                        ui.horizontal(|ui| {
                            if ui.button("用系统程序打开").clicked() {
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
}

impl AdramaApp {
    fn nav_button(&mut self, ui: &mut egui::Ui, tab: Tab, label: &str) {
        if ui.selectable_label(self.tab == tab, label).clicked() {
            self.tab = tab;
        }
    }

    fn ui_workflow(&mut self, ui: &mut egui::Ui) {
        if self.project.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("工作流画布");
                ui.label("请先打开或新建一个项目，然后在画布上编排流水线。");
                ui.add_space(12.0);
                if ui.button("前往项目总览").clicked() {
                    self.tab = Tab::Overview;
                }
            });
            return;
        }
        let action = self.workflow.show(
            ui,
            self.project.as_ref(),
            self.busy,
            self.busy_kind,
        );
        self.handle_wf_action(action);
    }

    fn ui_overview(&mut self, ui: &mut egui::Ui) {
        ui.heading("项目总览");
        ui.add_space(8.0);

        if self.project.is_none() {
            ui.label("创建新项目，或打开含 project.toml 的已有项目目录。");
            ui.add_space(12.0);

            if !self.app_settings.recent_projects.is_empty() {
                ui.group(|ui| {
                    ui.heading("最近项目");
                    let recent = self.app_settings.recent_projects.clone();
                    for p in recent {
                        ui.horizontal(|ui| {
                            ui.monospace(p.display().to_string());
                            if ui.button("打开").clicked() {
                                self.open_project(&p);
                            }
                        });
                    }
                });
                ui.add_space(10.0);
            }

            ui.group(|ui| {
                ui.heading("新建项目");
                ui.horizontal(|ui| {
                    ui.label("父目录：");
                    ui.label(self.create_parent.display().to_string());
                    if ui.button("浏览…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.create_parent = p;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("名称：");
                    ui.text_edit_singleline(&mut self.new_name);
                });
                ui.horizontal(|ui| {
                    ui.label("风格：");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_style).desired_width(480.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("画幅：");
                    ui.selectable_value(&mut self.new_aspect, "16:9".into(), "16:9 横屏");
                    ui.selectable_value(&mut self.new_aspect, "9:16".into(), "9:16 竖屏");
                    ui.selectable_value(&mut self.new_aspect, "1:1".into(), "1:1");
                });
                if ui.button("创建项目").clicked() {
                    self.create_project();
                }
            });
            return;
        }

        let (name, style, aspect, root, counts) = {
            let p = self.project.as_ref().unwrap();
            let bd = p.load_breakdown().ok();
            let counts = bd.map(|b| {
                (
                    b.characters.len(),
                    b.locations.len(),
                    b.scenes.len(),
                    b.shots.len(),
                )
            });
            (
                p.config.name.clone(),
                p.config.style.clone(),
                p.config.aspect.clone(),
                p.root.display().to_string(),
                counts,
            )
        };

        egui::Grid::new("proj_info")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label(RichText::new("名称").weak());
                ui.label(&name);
                ui.end_row();
                ui.label(RichText::new("路径").weak());
                ui.monospace(&root);
                ui.end_row();
                ui.label(RichText::new("风格").weak());
                ui.label(&style);
                ui.end_row();
                ui.label(RichText::new("画幅").weak());
                ui.label(&aspect);
                ui.end_row();
            });

        if let Some((c, l, s, sh)) = counts {
            ui.label(format!(
                "结构化：{c} 角色 · {l} 场景地 · {s} 场 · {sh} 镜头"
            ));
        }
        ui.label(format!(
            "产物：{} 张资产图 · {} 张分镜 · {} 个视频",
            self.asset_thumbs.len(),
            self.storyboard_thumbs.len(),
            self.video_files.len()
        ));

        ui.add_space(14.0);
        ui.heading("快捷操作");
        ui.horizontal_wrapped(|ui| {
            let en = !self.busy;
            if ui
                .add_enabled(en, egui::Button::new("导入剧本…"))
                .clicked()
            {
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
            if ui
                .add_enabled(en, egui::Button::new("打开工作流画布"))
                .clicked()
            {
                self.tab = Tab::Workflow;
            }
        });

        ui.add_space(10.0);
        ui.heading("阶段审核");
        ui.horizontal_wrapped(|ui| {
            for stage in Stage::all() {
                if ui
                    .add_enabled(
                        !self.busy,
                        egui::Button::new(format!("通过 · {}", stage_zh(*stage))),
                    )
                    .clicked()
                {
                    self.submit(Job::Approve { stage: *stage });
                }
            }
        });
    }

    fn ui_script(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("剧本");
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("导入…"),
                )
                .clicked()
            {
                self.import_script_dialog();
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new(if self.script_dirty {
                        "保存 *"
                    } else {
                        "保存"
                    }),
                )
                .clicked()
            {
                self.save_script();
            }
            if let Some(p) = &self.script_path {
                ui.label(RichText::new(p.display().to_string()).small().weak());
            }
        });
        ui.label(
            RichText::new("支持 .md / .txt / .fountain · 可直接编辑后保存到项目 script/ 目录")
                .weak(),
        );
        ui.separator();
        egui::ScrollArea::both().show(ui, |ui| {
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.script_text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(28)
                    .font(egui::TextStyle::Monospace),
            );
            if response.changed() {
                self.script_dirty = true;
            }
        });
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

    fn ui_parse(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("阶段 1 · 解析");
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("运行解析"),
                )
                .clicked()
            {
                self.submit(Job::Parse {
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("审核通过"),
                )
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Parse,
                });
            }
        });
        ui.label("使用对话模型将剧本解析为角色 / 场景 / 镜头结构（breakdown.json）。");
        ui.separator();

        if let Some(proj) = self.project.as_ref() {
            if let Ok(bd) = proj.load_breakdown() {
                self.ui_breakdown_summary(ui, &bd);
                ui.separator();
            }
        }

        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.breakdown_text)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace)
                    .interactive(false),
            );
        });
    }

    fn ui_breakdown_summary(&self, ui: &mut egui::Ui, bd: &Breakdown) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&bd.title).strong());
            if !bd.summary.is_empty() {
                ui.label(RichText::new(&bd.summary).italics());
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
        ui.horizontal(|ui| {
            ui.heading("阶段 2 · 资产");
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("全部生成"),
                )
                .clicked()
            {
                self.submit(Job::Assets {
                    only: None,
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("审核通过"),
                )
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Assets,
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label("仅生成名称/ID：");
            ui.add(egui::TextEdit::singleline(&mut self.only_asset).desired_width(160.0));
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some() && !self.only_asset.trim().is_empty(),
                    egui::Button::new("生成指定项"),
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
        ui.separator();
        self.ui_image_grid(ui, ctx, &self.asset_thumbs.clone());
    }

    fn ui_storyboard(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.heading("阶段 3 · 分镜");
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("全部生成"),
                )
                .clicked()
            {
                self.submit(Job::Storyboard {
                    scene: None,
                    shot: None,
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("审核通过"),
                )
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Storyboard,
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label("场次 #：");
            ui.add(egui::TextEdit::singleline(&mut self.scene_filter).desired_width(60.0));
            ui.label("镜头 ID：");
            ui.add(egui::TextEdit::singleline(&mut self.shot_filter).desired_width(120.0));
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("按条件生成"),
                )
                .clicked()
            {
                let scene = self.scene_filter.trim().parse().ok();
                let shot = nonempty(&self.shot_filter);
                self.submit(Job::Storyboard {
                    scene,
                    shot,
                    dry_run: self.dry_run,
                });
            }
        });
        self.ui_redo_row(ui, Stage::Storyboard);
        ui.separator();
        self.ui_image_grid(ui, ctx, &self.storyboard_thumbs.clone());
    }

    fn ui_video(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("阶段 4 · 视频");
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("全部生成"),
                )
                .clicked()
            {
                self.submit(Job::Video {
                    shot: None,
                    concat: self.video_concat,
                    dry_run: self.dry_run,
                });
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("审核通过"),
                )
                .clicked()
            {
                self.submit(Job::Approve {
                    stage: Stage::Video,
                });
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.video_concat, "生成后用 ffmpeg 拼接成片");
            ui.label("镜头 ID：");
            ui.add(egui::TextEdit::singleline(&mut self.shot_filter).desired_width(120.0));
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("生成指定镜头"),
                )
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
        ui.separator();
        if let Some(proj) = self.project.as_ref() {
            let final_mp4 = proj.video_dir().join("final.mp4");
            if final_mp4.exists() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(100, 200, 120), "成片 final.mp4 已生成");
                    if ui.button("播放成片").clicked() {
                        open_path(&final_mp4);
                    }
                });
            }
        }
        ui.label("视频文件（用系统播放器打开）：");
        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.video_files.is_empty() {
                ui.label(RichText::new("（暂无 mp4）").weak());
            }
            for path in self.video_files.clone() {
                ui.horizontal(|ui| {
                    ui.monospace(
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("?"),
                    );
                    if ui.button("打开").clicked() {
                        open_path(&path);
                    }
                    if ui.button("所在文件夹").clicked() {
                        if let Some(parent) = path.parent() {
                            open_path(parent);
                        }
                    }
                });
            }
        });
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置");
        ui.label(
            RichText::new(format!(
                "全局密钥文件：{}",
                AppSettings::config_path().display()
            ))
            .small()
            .weak(),
        );
        ui.add_space(8.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // ---- API Keys ----
            ui.group(|ui| {
                ui.label(RichText::new("API 密钥（本机持久化）").strong().size(16.0));
                ui.label(
                    RichText::new("密钥不会写入项目目录；仅保存在用户配置目录。")
                        .small()
                        .weak(),
                );
                ui.add_space(6.0);

                key_row(
                    ui,
                    "OpenAI / Image2 Key",
                    "OPENAI_API_KEY",
                    &mut self.app_settings.openai_api_key,
                    &mut self.settings_dirty,
                );
                key_row(
                    ui,
                    "Google / Gemini / Veo（Omni）Key",
                    "GEMINI_API_KEY",
                    &mut self.app_settings.gemini_api_key,
                    &mut self.settings_dirty,
                );
                key_row(
                    ui,
                    "xAI / Grok Key",
                    "XAI_API_KEY",
                    &mut self.app_settings.xai_api_key,
                    &mut self.settings_dirty,
                );
                key_row(
                    ui,
                    "自定义端点 Key",
                    "ADRAMA_CUSTOM_API_KEY",
                    &mut self.app_settings.custom_api_key,
                    &mut self.settings_dirty,
                );

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("保存密钥").clicked() {
                        self.save_app_settings();
                    }
                    if ui.button("应用到当前进程").clicked() {
                        self.app_settings.apply_to_env();
                        self.status_msg = "密钥已应用到当前会话".into();
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new("连接测试").strong());
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("测试 OpenAI / Image2"))
                        .clicked()
                    {
                        self.app_settings.apply_to_env();
                        self.submit(Job::TestEndpoint {
                            kind: "OpenAI / Image2".into(),
                            base_url: self.edit_config.openai_base_url.clone(),
                            api_key: self.app_settings.openai_api_key.clone(),
                            model: self.edit_config.openai_chat_model.clone(),
                        });
                    }
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("测试 Google / Veo"))
                        .clicked()
                    {
                        self.app_settings.apply_to_env();
                        self.submit(Job::TestEndpoint {
                            kind: "Google / Veo".into(),
                            base_url: self.edit_config.google_base_url.clone(),
                            api_key: self.app_settings.gemini_api_key.clone(),
                            model: self.edit_config.google_video_model.clone(),
                        });
                    }
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("测试 Grok / xAI"))
                        .clicked()
                    {
                        self.app_settings.apply_to_env();
                        self.submit(Job::TestEndpoint {
                            kind: "xAI / Grok".into(),
                            base_url: self.edit_config.xai_base_url.clone(),
                            api_key: self.app_settings.xai_api_key.clone(),
                            model: self.edit_config.xai_chat_model.clone(),
                        });
                    }
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("测试自定义端点"))
                        .clicked()
                    {
                        self.app_settings.apply_to_env();
                        self.submit(Job::TestEndpoint {
                            kind: "自定义".into(),
                            base_url: self.edit_config.custom_base_url.clone(),
                            api_key: self.app_settings.custom_api_key.clone(),
                            model: self.edit_config.custom_chat_model.clone(),
                        });
                    }
                });
            });

            ui.add_space(12.0);

            // ---- Project endpoints ----
            if self.project.is_some() {
                ui.group(|ui| {
                    ui.label(
                        RichText::new("项目模型与端点（project.toml）")
                            .strong()
                            .size(16.0),
                    );
                    ui.label(
                        RichText::new(
                            "可分别配置 Image2、Omni/Veo、Grok 的 Base URL 与模型名；\
                             支持 OpenAI 兼容代理。",
                        )
                        .small()
                        .weak(),
                    );
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("项目名：");
                        if ui
                            .text_edit_singleline(&mut self.edit_config.name)
                            .changed()
                        {
                            self.settings_dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("风格前缀：");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.edit_config.style)
                                    .desired_width(420.0),
                            )
                            .changed()
                        {
                            self.settings_dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("画幅：");
                        for a in ["16:9", "9:16", "1:1"] {
                            if ui
                                .selectable_value(
                                    &mut self.edit_config.aspect,
                                    a.to_string(),
                                    a,
                                )
                                .changed()
                            {
                                self.settings_dirty = true;
                            }
                        }
                    });

                    ui.separator();
                    ui.label(RichText::new("能力路由（用哪家服务）").strong());
                    provider_row(
                        ui,
                        "对话 / 剧本解析",
                        &mut self.edit_config.chat_provider,
                        &mut self.settings_dirty,
                    );
                    provider_row(
                        ui,
                        "图像（资产 / 分镜）",
                        &mut self.edit_config.image_provider,
                        &mut self.settings_dirty,
                    );
                    provider_row(
                        ui,
                        "视频（图生视频）",
                        &mut self.edit_config.video_provider,
                        &mut self.settings_dirty,
                    );

                    ui.separator();
                    ui.collapsing(
                        RichText::new("① OpenAI / Image2（图片与对话）").strong(),
                        |ui| {
                            field(
                                ui,
                                "Base URL",
                                &mut self.edit_config.openai_base_url,
                                460.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "对话模型",
                                &mut self.edit_config.openai_chat_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "图像模型（Image2）",
                                &mut self.edit_config.openai_image_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            ui.label(
                                RichText::new(
                                    "示例：https://api.openai.com/v1  或兼容代理地址",
                                )
                                .small()
                                .weak(),
                            );
                        },
                    );

                    ui.collapsing(
                        RichText::new("② Google / Gemini / Veo（Omni 视频）").strong(),
                        |ui| {
                            field(
                                ui,
                                "Base URL",
                                &mut self.edit_config.google_base_url,
                                460.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "视频模型",
                                &mut self.edit_config.google_video_model,
                                320.0,
                                &mut self.settings_dirty,
                            );
                            ui.label(
                                RichText::new(
                                    "示例：https://generativelanguage.googleapis.com/v1beta\n\
                                     模型：veo-3.1-generate-preview（以官方文档为准）",
                                )
                                .small()
                                .weak(),
                            );
                        },
                    );

                    ui.collapsing(
                        RichText::new("③ xAI / Grok（图片 · 对话 · 视频）").strong(),
                        |ui| {
                            field(
                                ui,
                                "Base URL（OpenAI 兼容）",
                                &mut self.edit_config.xai_base_url,
                                460.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "对话模型",
                                &mut self.edit_config.xai_chat_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "图像模型",
                                &mut self.edit_config.xai_image_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "视频模型",
                                &mut self.edit_config.xai_video_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "视频 Base URL（可空=同上）",
                                &mut self.edit_config.xai_video_base_url,
                                460.0,
                                &mut self.settings_dirty,
                            );
                            ui.label(
                                RichText::new(
                                    "默认：https://api.x.ai/v1  ·  需填写 XAI_API_KEY / Grok Key",
                                )
                                .small()
                                .weak(),
                            );
                        },
                    );

                    ui.collapsing(
                        RichText::new("④ 自定义 OpenAI 兼容端点").strong(),
                        |ui| {
                            field(
                                ui,
                                "Base URL",
                                &mut self.edit_config.custom_base_url,
                                460.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "对话模型",
                                &mut self.edit_config.custom_chat_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "图像模型",
                                &mut self.edit_config.custom_image_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "视频模型",
                                &mut self.edit_config.custom_video_model,
                                280.0,
                                &mut self.settings_dirty,
                            );
                            field(
                                ui,
                                "视频 Base URL（可空）",
                                &mut self.edit_config.custom_video_base_url,
                                460.0,
                                &mut self.settings_dirty,
                            );
                        },
                    );

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let label = if self.settings_dirty {
                            "保存 project.toml *"
                        } else {
                            "保存 project.toml"
                        };
                        if ui.button(label).clicked() {
                            self.save_config();
                        }
                        if ui.button("从磁盘重新加载").clicked() {
                            self.reload_project();
                        }
                    });
                });
            } else {
                ui.group(|ui| {
                    ui.label("打开项目后，可在此编辑各厂商 Base URL 与模型 ID。");
                });
            }
        });
    }

    fn ui_redo_row(&mut self, ui: &mut egui::Ui, stage: Stage) {
        ui.horizontal(|ui| {
            ui.label("重生成 ID：");
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
            ui.label(RichText::new("（暂无图片）").weak());
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            let thumb = 168.0;
            ui.horizontal_wrapped(|ui| {
                for path in paths {
                    ui.allocate_ui_with_layout(
                        Vec2::new(thumb + 16.0, thumb + 52.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            if let Some(tex) = self.texture_for(ctx, path) {
                                let size = tex.size_vec2();
                                let scale = (thumb / size.x).min(thumb / size.y).min(1.0);
                                let img = egui::Image::new((tex.id(), size * scale))
                                    .maintain_aspect_ratio(true);
                                if ui
                                    .add(img)
                                    .on_hover_text(format!(
                                        "{}\n单击预览 · 双击用系统程序打开",
                                        path.display()
                                    ))
                                    .clicked()
                                {
                                    self.preview_image = Some(path.clone());
                                }
                                if ui.input(|i| {
                                    i.pointer
                                        .button_double_clicked(egui::PointerButton::Primary)
                                }) {
                                    // double-click handled via secondary button below
                                }
                            } else {
                                ui.allocate_exact_size(Vec2::splat(thumb), egui::Sense::hover());
                                ui.label("?");
                            }
                            let name = path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("?");
                            ui.label(RichText::new(name).small());
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
                                    RichText::new(item_status_zh(st)).small().weak(),
                                );
                            }
                        },
                    );
                }
            });
        });
    }
}

fn key_row(
    ui: &mut egui::Ui,
    label: &str,
    env_hint: &str,
    value: &mut String,
    dirty: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.set_min_width(220.0);
        ui.label(label);
        if ui
            .add(
                egui::TextEdit::singleline(value)
                    .password(true)
                    .desired_width(360.0)
                    .hint_text(env_hint),
            )
            .changed()
        {
            *dirty = true;
        }
    });
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32, dirty: &mut bool) {
    ui.horizontal(|ui| {
        ui.set_min_width(160.0);
        ui.label(label);
        if ui
            .add(egui::TextEdit::singleline(value).desired_width(width))
            .changed()
        {
            *dirty = true;
        }
    });
}

fn provider_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut ProviderKind,
    dirty: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.set_min_width(160.0);
        ui.label(label);
        for p in [
            ProviderKind::OpenAi,
            ProviderKind::Google,
            ProviderKind::Xai,
            ProviderKind::Custom,
        ] {
            if ui
                .selectable_value(value, p, p.label_zh())
                .changed()
            {
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
