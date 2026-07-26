//! Application shell: top bar, navigation rail, console drawer, view routing.
//! Content lives in `views`; state transitions live in `state`.

use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::time::Duration;

use super::runtime::Runtime;
use super::state::{AppState, View};
use super::thumbs::Thumbnails;
use super::views::{self, ViewCtx};
use super::{theme, widgets};
use crate::engine::events::Level;
use crate::engine::Job;
use crate::model::{Project, Stage};
use crate::settings::AppSettings;

pub struct AdramaApp {
    state: AppState,
    runtime: Runtime,
    thumbs: Thumbnails,
}

impl AdramaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: PathBuf) -> Self {
        let settings = AppSettings::load();
        let runtime = Runtime::spawn(cc.egui_ctx.clone());
        let mut state = AppState::new(settings);

        // Open the directory we were pointed at, else the last one used.
        let candidate = [Some(initial.clone()), state.settings.last_project.clone()]
            .into_iter()
            .flatten()
            .find(|p| Project::is_project(p));
        if let Some(path) = candidate {
            // `open_project` jumps to the overview, which is right when the user
            // switches projects but not when we are restoring last session.
            let restored = state.view;
            state.open_project(&runtime, &path);
            state.view = restored;
        }

        // 每天最多问一次 GitHub，失败也不打扰。
        if state.settings.update_check_due() {
            state.start_update_check(&runtime);
        }

        Self {
            state,
            runtime,
            thumbs: Thumbnails::default(),
        }
    }

    fn drain_updates(&mut self) {
        while let Ok(update) = self.runtime.rx.try_recv() {
            self.state.apply(update);
        }
        for path in std::mem::take(&mut self.state.dirty_thumbs) {
            self.thumbs.invalidate(&path);
        }
    }

    fn ctx(&mut self) -> ViewCtx<'_> {
        ViewCtx {
            state: &mut self.state,
            runtime: &self.runtime,
            thumbs: &mut self.thumbs,
        }
    }
}

impl eframe::App for AdramaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_updates();
        self.state.watch_for_stalls();
        self.thumbs.begin_frame();

        // 有任务在排队时保持重绘，否则卡住的提示要等下次交互才出现。
        if self.state.is_busy() || self.state.updates.checking {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        if self.state.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        self.top_bar(ctx);
        self.nav_rail(ctx);
        self.console(ctx);

        egui::CentralPanel::default()
            .frame(theme::content())
            .show(ctx, |ui| {
                let view = self.state.view;
                let mut cx = self.ctx();
                match view {
                    View::Dashboard => views::dashboard::show(ui, &mut cx),
                    View::Script => views::script::show(ui, &mut cx),
                    View::Flow => views::flow::show(ui, &mut cx),
                    View::Audio => views::audio::show(ui, &mut cx),
                    View::Stage(Stage::Parse) => views::breakdown::show(ui, &mut cx),
                    View::Stage(stage) => views::workbench::show(ui, &mut cx, stage),
                    View::Settings => views::settings::show(ui, &mut cx),
                }
            });

        self.preview_window(ctx);

        if self.thumbs.has_pending() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.persist_ui_prefs();
    }
}

impl AdramaApp {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("topbar")
            .exact_height(54.0)
            .frame(theme::bar())
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("adrama")
                            .size(17.0)
                            .strong()
                            .color(theme::ACCENT),
                    );

                    match &self.state.snapshot {
                        Some(snapshot) => {
                            ui.add_space(theme::SPACE_SM);
                            widgets::dot(ui, theme::stage_color(snapshot.state.current_stage()), 8.0);
                            ui.label(RichText::new(&snapshot.config.name).strong());
                            widgets::path_label(ui, &snapshot.root);
                        }
                        None => {
                            ui.label(RichText::new("未打开项目").color(theme::TEXT_MUTED));
                        }
                    }

                    ui.add_space(theme::SPACE_SM);
                    if widgets::button(ui, "打开…", !self.state.is_busy()) {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.state.open_project(&self.runtime, &path);
                        }
                    }
                    if widgets::button(
                        ui,
                        "刷新",
                        !self.state.is_busy() && self.state.snapshot.is_some(),
                    ) {
                        self.state.refresh(&self.runtime);
                    }

                    self.dry_run_badge(ui);
                    self.update_badge(ui);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.busy_indicator(ui);
                    });
                });
            });

        self.banner(ctx);
    }

    /// 演练模式很容易忘了关，忘了就会「跑完但什么都没生成」。
    fn dry_run_badge(&mut self, ui: &mut egui::Ui) {
        if !self.state.dry_run {
            return;
        }
        let clicked = widgets::pill(ui, "演练模式 · 不会调用模型", theme::WARNING)
            .interact(egui::Sense::click())
            .on_hover_text("点击退出演练模式")
            .clicked();
        if clicked {
            self.state.dry_run = false;
            self.state.persist_ui_prefs();
        }
    }

    /// 有新版本时在顶栏挂一个可点的徽标。
    fn update_badge(&mut self, ui: &mut egui::Ui) {
        let Some(version) = self.state.updates.available().map(str::to_string) else {
            return;
        };
        let clicked = widgets::pill(ui, &format!("有新版本 {version}"), theme::WARNING)
            .interact(egui::Sense::click())
            .on_hover_text("点击查看更新说明并安装")
            .clicked();
        if clicked {
            self.state.view = View::Settings;
            self.state.settings_tab = super::state::SettingsTab::About;
        }
    }

    fn busy_indicator(&mut self, ui: &mut egui::Ui) {
        let busy = self
            .state
            .busy
            .as_ref()
            .map(|b| (b.label.clone(), b.progress.clone(), b.started.elapsed()));
        match busy {
            Some((label, progress, elapsed)) => {
                if widgets::danger_button(ui, "取消", true) {
                    self.runtime.cancel();
                    self.state
                        .push_console(Level::Warn, "已请求取消，正在等待当前请求返回…");
                }
                if let Some((done, total, detail)) = &progress {
                    if *total > 0 {
                        ui.add(
                            egui::ProgressBar::new(*done as f32 / *total as f32)
                                .desired_width(150.0)
                                .text(format!("{done}/{total}")),
                        );
                    }
                    ui.label(
                        RichText::new(crate::model::index::truncate(detail, 34))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                ui.label(
                    RichText::new(format!("{}s", elapsed.as_secs()))
                        .small()
                        .monospace()
                        .color(theme::TEXT_DIM),
                );
                ui.label(RichText::new(label).color(theme::INFO));
                ui.spinner();
            }
            None => {
                let mut dry = self.state.dry_run;
                if ui
                    .checkbox(&mut dry, "演练模式")
                    .on_hover_text("只组装并展示 prompt，不调用任何付费接口")
                    .changed()
                {
                    self.state.dry_run = dry;
                    self.state.persist_ui_prefs();
                }
            }
        }
    }

    /// Result of the last job, directly under the top bar. Successes fade on
    /// their own; failures stay until dismissed so nothing is missed.
    fn banner(&mut self, ctx: &egui::Context) {
        const AUTO_DISMISS: Duration = Duration::from_secs(8);
        let Some(banner) = &self.state.banner else {
            return;
        };
        if banner.ok && banner.at.elapsed() >= AUTO_DISMISS {
            self.state.banner = None;
            return;
        }
        if banner.ok {
            ctx.request_repaint_after(AUTO_DISMISS - banner.at.elapsed());
        }
        let color = if banner.ok { theme::SUCCESS } else { theme::DANGER };
        let text = banner.text.clone();
        let detail = banner.detail.clone();
        let mut dismiss = false;

        egui::TopBottomPanel::top("banner")
            .frame(
                egui::Frame::new()
                    .fill(theme::tint(color, 26))
                    .stroke(egui::Stroke::new(1.0_f32, theme::tint(color, 90)))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(if banner.ok { "✔" } else { "✖" }).color(color));
                    ui.label(RichText::new(text).color(theme::TEXT));
                    if let Some(detail) = detail {
                        ui.label(
                            RichText::new(crate::model::index::truncate(&detail, 90))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("关闭").clicked() {
                            dismiss = true;
                        }
                    });
                });
            });

        if dismiss {
            self.state.banner = None;
        }
    }

    fn nav_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("rail")
            .exact_width(206.0)
            .resizable(false)
            .frame(theme::rail())
            .show(ctx, |ui| {
                self.nav_item(ui, View::Dashboard, "概览", None);
                self.nav_item(ui, View::Script, "剧本", None);
                self.nav_item(ui, View::Flow, "流程图", None);

                ui.add_space(theme::SPACE_MD);
                widgets::hint(ui, "生产阶段");
                ui.add_space(theme::SPACE_XS);
                for stage in Stage::ALL {
                    self.nav_item(ui, View::Stage(stage), stage.label(), Some(stage));
                }
                self.nav_item(ui, View::Audio, "配音与字幕", None);

                ui.add_space(theme::SPACE_MD);
                self.nav_item(ui, View::Settings, "设置", None);

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(theme::SPACE_SM);
                    let mut open = self.state.console_open;
                    if ui.checkbox(&mut open, "显示控制台").changed() {
                        self.state.console_open = open;
                        self.state.persist_ui_prefs();
                    }
                    if let Some(snapshot) = &self.state.snapshot {
                        let root = snapshot.root.clone();
                        if ui.button("打开项目目录").clicked() {
                            widgets::open_path(&root);
                        }
                    }
                });
            });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, view: View, label: &str, stage: Option<Stage>) {
        let selected = self.state.view == view;
        let status = stage.and_then(|s| self.state.snapshot.as_ref().map(|snap| snap.state.get(s)));
        let counts = stage
            .filter(|s| *s != Stage::Parse)
            .and_then(|s| self.state.snapshot.as_ref().map(|snap| snap.index.counts(s)));

        let response = widgets::selectable_row(ui, selected, |ui| {
            ui.horizontal(|ui| {
                let accent = stage.map(theme::stage_color).unwrap_or(theme::TEXT_DIM);
                if selected {
                    theme::accent_bar(ui, accent, 16.0);
                    ui.add_space(2.0);
                } else {
                    widgets::dot(ui, accent, 7.0);
                }
                ui.label(
                    RichText::new(label)
                        .strong()
                        .color(if selected { theme::TEXT } else { theme::TEXT_MUTED }),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(status) = status {
                        ui.label(
                            RichText::new(status.glyph())
                                .small()
                                .color(theme::stage_status_color(status)),
                        );
                    }
                    if let Some(counts) = counts {
                        if counts.total > 0 {
                            ui.label(
                                RichText::new(format!("{}/{}", counts.ready, counts.total))
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                        }
                    }
                });
            });
        });

        if response.clicked() {
            self.state.view = view;
        }
    }

    fn console(&mut self, ctx: &egui::Context) {
        if !self.state.console_open {
            return;
        }
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(170.0)
            .min_height(90.0)
            .max_height(420.0)
            .frame(theme::bar())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    widgets::section_title(ui, "控制台");
                    widgets::hint(ui, &format!("{} 条", self.state.console.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("收起").clicked() {
                            self.state.console_open = false;
                        }
                        if ui.small_button("清空").clicked() {
                            self.state.console.clear();
                        }
                    });
                });
                ui.add_space(theme::SPACE_XS);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        // 控制台里的文字要能选中复制——报错就是拿来贴给别人看的。
                        ui.style_mut().interaction.selectable_labels = true;
                        ui.spacing_mut().item_spacing.y = 3.0;
                        if self.state.console.is_empty() {
                            widgets::hint(ui, "运行任务后，进度与错误会显示在这里。");
                        }
                        for line in &self.state.console {
                            let (glyph, glyph_color, text_color) = match line.level {
                                Level::Info => ("·", theme::TEXT_DIM, theme::TEXT_MUTED),
                                Level::Warn => ("!", theme::WARNING, theme::WARNING),
                                Level::Error => ("✖", theme::DANGER, theme::DANGER),
                            };
                            ui.horizontal_top(|ui| {
                                ui.label(
                                    RichText::new(glyph)
                                        .monospace()
                                        .size(12.0)
                                        .color(glyph_color),
                                );
                                ui.label(
                                    RichText::new(&line.text)
                                        .monospace()
                                        .size(12.0)
                                        .color(text_color),
                                );
                            });
                        }
                    });
            });
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(path) = self.state.preview.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new(
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("预览")
                .to_string(),
        )
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([880.0, 620.0])
        .frame(theme::card())
        .show(ctx, |ui| {
            widgets::path_label(ui, &path);
            ui.separator();
            let available = ui.available_size() - egui::vec2(0.0, 40.0);
            if let Some(texture) = self.thumbs.get(ctx, &path, 1600) {
                let size = texture.size_vec2();
                let scale = (available.x / size.x).min(available.y / size.y).min(1.0);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new((texture.id(), size * scale.max(0.05))));
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("加载中…").color(theme::TEXT_MUTED));
                });
            }
            ui.horizontal(|ui| {
                if widgets::button(ui, "用系统程序打开", true) {
                    widgets::open_path(&path);
                }
                if let Some(parent) = path.parent() {
                    if widgets::button(ui, "打开所在目录", true) {
                        widgets::open_path(parent);
                    }
                }
            });
        });

        if !open {
            self.state.preview = None;
        }
    }
}

/// Shared helper: run a stage from anywhere in the UI.
pub fn run_stage(cx: &mut ViewCtx<'_>, stage: Stage) {
    let job = match stage {
        Stage::Parse => Job::Parse,
        Stage::Assets => Job::Assets(Default::default()),
        Stage::Storyboard => Job::Storyboard(Default::default()),
        Stage::Video => Job::Video(Default::default()),
    };
    cx.state.submit(cx.runtime, job);
}
