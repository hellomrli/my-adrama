//! All mutable UI state, plus how engine updates fold into it.
//! Keeping this out of `app.rs` leaves that file as pure layout.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::runtime::{Runtime, Snapshot, Update};
use crate::engine::events::{Level, StageEvent};
use crate::engine::{Job, JobOutcome};
use crate::model::{ItemStatus, ItemView, ProjectConfig, Stage};
use crate::providers::Credentials;
use crate::settings::AppSettings;

const CONSOLE_CAPACITY: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Dashboard,
    Script,
    /// 由 breakdown 推导出的依赖图。
    Flow,
    Stage(Stage),
    Settings,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Dashboard => "概览",
            View::Script => "剧本",
            View::Flow => "流程图",
            View::Stage(Stage::Parse) => "拆解",
            View::Stage(Stage::Assets) => "资产",
            View::Stage(Stage::Storyboard) => "分镜",
            View::Stage(Stage::Video) => "视频",
            View::Settings => "设置",
        }
    }

    /// Stable key used to remember the current screen across sessions.
    pub fn key(self) -> String {
        match self {
            View::Dashboard => "dashboard".into(),
            View::Script => "script".into(),
            View::Flow => "flow".into(),
            View::Stage(stage) => format!("stage:{stage}"),
            View::Settings => "settings".into(),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "dashboard" => Some(View::Dashboard),
            "script" => Some(View::Script),
            "flow" => Some(View::Flow),
            "settings" => Some(View::Settings),
            other => other
                .strip_prefix("stage:")
                .and_then(|s| s.parse::<Stage>().ok())
                .map(View::Stage),
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            View::Dashboard => "项目状态与下一步操作",
            View::Script => "导入或直接编写剧本原文",
            View::Flow => "这个项目接下来会做哪些事、彼此依赖关系如何",
            View::Stage(Stage::Parse) => "模型拆出的角色、场次与镜头表",
            View::Stage(Stage::Assets) => "角色定妆照与服化道参考图",
            View::Stage(Stage::Storyboard) => "逐镜头画面，复用资产保持一致性",
            View::Stage(Stage::Video) => "分镜图生成视频片段并拼接成片",
            View::Settings => "按能力配置供应商、密钥与模型",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// 按能力组织的供应商 / 端点 / 密钥 / 模型。
    Models,
    Project,
    About,
}

/// In-app updater state.
pub struct UpdateState {
    pub install: crate::update::InstallKind,
    pub checking: bool,
    pub last_result: Option<Result<crate::update::UpdateStatus, String>>,
    /// `(已下载, 总量)` while a download is in flight.
    pub download: Option<(u64, u64)>,
    pub applied: Option<crate::update::Applied>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            install: crate::update::install_kind(),
            checking: false,
            last_result: None,
            download: None,
            applied: None,
        }
    }
}

impl UpdateState {
    /// Version string when a newer release is known to exist.
    pub fn available(&self) -> Option<&str> {
        match &self.last_result {
            Some(Ok(crate::update::UpdateStatus::Available(release))) => {
                Some(release.version.as_str())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemFilter {
    All,
    Pending,
    Failed,
}

impl ItemFilter {
    pub const ALL: [ItemFilter; 3] = [ItemFilter::All, ItemFilter::Pending, ItemFilter::Failed];

    pub fn label(self) -> &'static str {
        match self {
            ItemFilter::All => "全部",
            ItemFilter::Pending => "待生成",
            ItemFilter::Failed => "失败",
        }
    }

    pub fn accepts(self, status: ItemStatus) -> bool {
        match self {
            ItemFilter::All => true,
            ItemFilter::Pending => matches!(status, ItemStatus::Pending | ItemStatus::Generating),
            ItemFilter::Failed => status == ItemStatus::Failed,
        }
    }
}

pub struct Busy {
    pub label: String,
    pub progress: Option<(u32, u32, String)>,
    pub started: Instant,
}

pub struct ConsoleLine {
    pub level: Level,
    pub text: String,
}

/// Non-blocking result banner shown under the top bar.
pub struct Banner {
    pub ok: bool,
    pub text: String,
    pub detail: Option<String>,
    pub at: Instant,
}

#[derive(Default)]
pub struct NewProjectForm {
    pub parent: Option<PathBuf>,
    pub name: String,
    pub style: String,
    pub aspect: crate::model::AspectRatio,
}

/// Prompt being edited in the item inspector.
pub struct PromptEdit {
    pub stage: Stage,
    pub id: String,
    pub text: String,
    pub dirty: bool,
}

pub struct AppState {
    pub view: View,
    pub settings_tab: SettingsTab,
    pub snapshot: Option<Snapshot>,
    pub busy: Option<Busy>,
    pub console: VecDeque<ConsoleLine>,
    pub console_open: bool,
    pub banner: Option<Banner>,

    /// Per-item status published by a running job, before the disk rescan.
    pub live_status: HashMap<(Stage, String), (ItemStatus, String)>,
    pub selection: HashMap<Stage, String>,
    /// 勾选待生成的条目（每个阶段一份）。
    pub checked: HashMap<Stage, std::collections::BTreeSet<String>>,
    pub item_filter: ItemFilter,
    pub prompt_edit: Option<PromptEdit>,
    pub thumb_size: f32,
    /// Show `breakdown.json` verbatim instead of the structured tables.
    pub raw_breakdown: bool,

    pub script_text: String,
    pub script_dirty: bool,

    pub settings: AppSettings,
    pub config_draft: ProjectConfig,
    pub config_dirty: bool,
    pub keys_dirty: bool,
    pub revealed: HashMap<String, bool>,

    /// 流程图的平移与缩放。
    pub flow_pan: eframe::egui::Vec2,
    pub flow_zoom: f32,
    /// 下一帧把整张图缩放到刚好放得下。
    pub flow_fit: bool,

    pub new_project: NewProjectForm,
    pub preview: Option<PathBuf>,
    pub dry_run: bool,
    pub updates: UpdateState,
    /// 任务已提交但后台还没开始的时刻——用来在卡住时喊一声。
    pending_since: Option<Instant>,
    pending_warned: bool,
    /// Files a running job just wrote; the app drops their cached textures so
    /// a regenerated image appears immediately.
    pub dirty_thumbs: Vec<PathBuf>,
}

impl AppState {
    pub fn new(settings: AppSettings) -> Self {
        let dry_run = settings.ui.dry_run;
        let thumb_size = settings.ui.thumbnail_size;
        let console_open = settings.ui.console_open;
        let view = settings
            .ui
            .last_view
            .as_deref()
            .and_then(View::from_key)
            .unwrap_or(View::Dashboard);
        Self {
            view,
            settings_tab: SettingsTab::Models,
            snapshot: None,
            busy: None,
            console: VecDeque::new(),
            console_open,
            banner: None,
            live_status: HashMap::new(),
            selection: HashMap::new(),
            checked: HashMap::new(),
            item_filter: ItemFilter::All,
            prompt_edit: None,
            thumb_size,
            raw_breakdown: false,
            script_text: String::new(),
            script_dirty: false,
            settings,
            config_draft: ProjectConfig::default(),
            config_dirty: false,
            keys_dirty: false,
            revealed: HashMap::new(),
            flow_pan: eframe::egui::Vec2::new(24.0, 16.0),
            flow_zoom: 1.0,
            flow_fit: true,
            new_project: NewProjectForm {
                parent: std::env::current_dir().ok(),
                name: "my-drama".into(),
                style: "cinematic, photorealistic, film grain".into(),
                aspect: crate::model::AspectRatio::Landscape,
            },
            preview: None,
            dry_run,
            updates: UpdateState::default(),
            pending_since: None,
            pending_warned: false,
            dirty_thumbs: Vec::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.snapshot.as_ref().map(|s| s.root.clone())
    }

    pub fn credentials(&self) -> Credentials {
        self.settings.credentials()
    }

    pub fn push_console(&mut self, level: Level, text: impl Into<String>) {
        self.console.push_back(ConsoleLine {
            level,
            text: text.into(),
        });
        while self.console.len() > CONSOLE_CAPACITY {
            self.console.pop_front();
        }
    }

    pub fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.push_console(Level::Info, text.clone());
        self.banner = Some(Banner {
            ok: true,
            text,
            detail: None,
            at: Instant::now(),
        });
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.push_console(Level::Error, text.clone());
        self.banner = Some(Banner {
            ok: false,
            text,
            detail: None,
            at: Instant::now(),
        });
    }

    /// Fold one worker message into UI state.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Event(event) => self.apply_event(event),
            Update::Snapshot(snapshot) => self.adopt(*snapshot),
            Update::ScanFailed(err) => self.fail(format!("读取项目失败：{err}")),
            Update::Outcome(Ok(outcome)) => self.apply_outcome(outcome),
            Update::Outcome(Err(err)) => self.fail(err),
            Update::NewVersion(result) => {
                self.updates.checking = false;
                match &result {
                    Ok(crate::update::UpdateStatus::Available(release)) => {
                        self.note(format!("发现新版本 {}（设置 → 关于与更新）", release.version));
                    }
                    Ok(crate::update::UpdateStatus::UpToDate) => {
                        self.push_console(Level::Info, "已是最新版本");
                    }
                    Err(err) => self.push_console(Level::Warn, format!("检查更新失败：{err}")),
                }
                self.updates.last_result = Some(result);
                self.settings.mark_update_checked();
                let _ = self.settings.save();
            }
            Update::DownloadProgress { received, total } => {
                self.updates.download = Some((received, total));
            }
            Update::Installed(result) => {
                self.updates.download = None;
                match result {
                    Ok(applied) => {
                        self.note(format!("已更新到 {}，重启后生效", applied.version));
                        self.updates.applied = Some(applied);
                    }
                    Err(err) => self.fail(format!("更新失败：{err}")),
                }
            }
        }
    }

    // --- updater ------------------------------------------------------------

    pub fn start_update_check(&mut self, runtime: &Runtime) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        runtime.check_update();
    }

    pub fn start_update_download(&mut self, runtime: &Runtime) {
        let release = match &self.updates.last_result {
            Some(Ok(crate::update::UpdateStatus::Available(release))) => (**release).clone(),
            _ => return,
        };
        if self.updates.download.is_some() {
            return;
        }
        self.updates.download = Some((0, 0));
        self.push_console(Level::Info, format!("开始下载 {}", release.version));
        runtime.apply_update(release);
    }

    /// Launch the freshly installed binary and close this window.
    pub fn restart_after_update(&mut self, ctx: &eframe::egui::Context) {
        let Some(applied) = &self.updates.applied else {
            return;
        };
        match crate::update::restart(&applied.executable) {
            Ok(()) => ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close),
            Err(err) => self.fail(format!("重启失败：{err:#}")),
        }
    }

    fn apply_event(&mut self, event: StageEvent) {
        match event {
            StageEvent::Started { label } => {
                self.pending_since = None;
                self.pending_warned = false;
                self.live_status.clear();
                self.busy = Some(Busy {
                    label: label.clone(),
                    progress: None,
                    started: Instant::now(),
                });
                self.push_console(Level::Info, format!("▶ {label}"));
            }
            StageEvent::Log { level, message } => self.push_console(level, message),
            StageEvent::Progress { done, total, detail } => {
                if let Some(busy) = &mut self.busy {
                    busy.progress = Some((done, total, detail));
                }
            }
            StageEvent::Item {
                stage,
                id,
                status,
                detail,
            } => {
                self.live_status.insert((stage, id), (status, detail));
            }
            StageEvent::Artifact { path } => self.dirty_thumbs.push(path),
            StageEvent::Finished { ok, message } => {
                self.busy = None;
                self.pending_since = None;
                self.push_console(
                    if ok { Level::Info } else { Level::Error },
                    format!("{} {message}", if ok { "✔" } else { "✖" }),
                );
                self.banner = Some(Banner {
                    ok,
                    text: message,
                    detail: None,
                    at: Instant::now(),
                });
            }
        }
    }

    fn apply_outcome(&mut self, outcome: JobOutcome) {
        // A connectivity probe doubles as "fetch the model list".
        if let Some(probed) = outcome.models {
            let count = probed.models.len();
            self.settings.set_known_models(
                probed.capability,
                probed.provider,
                probed.mode,
                probed.models,
            );
            let _ = self.settings.save();
            if count > 0 {
                self.push_console(
                    Level::Info,
                    format!(
                        "{}：{} {} 可用模型 {count} 个，已写入下拉列表",
                        probed.capability.label(),
                        probed.provider.label(),
                        probed.mode.label()
                    ),
                );
            }
        }
        if let Some(detail) = outcome.detail {
            self.push_console(Level::Info, detail.clone());
            if let Some(banner) = &mut self.banner {
                banner.detail = Some(detail);
            }
        }
    }

    fn adopt(&mut self, snapshot: Snapshot) {
        // Never clobber unsaved edits with what is on disk.
        if !self.script_dirty {
            self.script_text = snapshot.script_text.clone();
        }
        if !self.config_dirty {
            self.config_draft = snapshot.config.clone();
        }
        if !self.is_busy() {
            self.live_status.clear();
        }
        // Drop a prompt editor whose item vanished.
        if let Some(edit) = &self.prompt_edit {
            if !edit.dirty && snapshot.index.find(edit.stage, &edit.id).is_none() {
                self.prompt_edit = None;
            }
        }
        if self.settings.last_project.as_ref() != Some(&snapshot.root) {
            self.settings.remember_project(snapshot.root.clone());
            let _ = self.settings.save();
        }
        self.snapshot = Some(snapshot);
    }

    // --- item helpers ------------------------------------------------------

    pub fn items(&self, stage: Stage) -> &[ItemView] {
        self.snapshot
            .as_ref()
            .map(|s| s.index.items(stage))
            .unwrap_or(&[])
    }

    /// Live status from a running job wins over the last disk scan.
    pub fn item_status(&self, stage: Stage, item: &ItemView) -> (ItemStatus, Option<String>) {
        match self.live_status.get(&(stage, item.id.clone())) {
            Some((status, detail)) => (*status, Some(detail.clone())),
            None => (item.status, None),
        }
    }

    /// 该阶段被勾选的条目。
    pub fn checked_ids(&self, stage: Stage) -> Vec<String> {
        self.checked
            .get(&stage)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn is_checked(&self, stage: Stage, id: &str) -> bool {
        self.checked.get(&stage).is_some_and(|set| set.contains(id))
    }

    pub fn toggle_checked(&mut self, stage: Stage, id: &str) {
        let set = self.checked.entry(stage).or_default();
        if !set.remove(id) {
            set.insert(id.to_string());
        }
    }

    pub fn clear_checked(&mut self, stage: Stage) {
        self.checked.remove(&stage);
    }

    pub fn selected_id(&self, stage: Stage) -> Option<&str> {
        self.selection.get(&stage).map(|s| s.as_str())
    }

    pub fn selected_item(&self, stage: Stage) -> Option<&ItemView> {
        let id = self.selected_id(stage)?;
        self.items(stage).iter().find(|i| i.id == id)
    }

    pub fn select(&mut self, stage: Stage, item: &ItemView) {
        self.selection.insert(stage, item.id.clone());
        let already = self
            .prompt_edit
            .as_ref()
            .map(|e| e.stage == stage && e.id == item.id)
            .unwrap_or(false);
        if !already {
            self.prompt_edit = Some(PromptEdit {
                stage,
                id: item.id.clone(),
                text: item.prompt.clone(),
                dirty: false,
            });
        }
    }

    // --- actions -----------------------------------------------------------

    pub fn submit(&mut self, runtime: &Runtime, job: Job) {
        let Some(root) = self.root() else {
            self.fail("尚未打开项目");
            return;
        };
        if self.is_busy() {
            self.fail("已有任务在运行，请先等待或取消");
            return;
        }
        // 任务从磁盘读 project.toml。界面里刚改的服务商/模型如果还没保存，
        // 跑起来用的就会是旧配置——所以先落盘，别让人对着改好的界面纳闷。
        if !self.flush_pending_edits() {
            return;
        }

        // 先在控制台留一行：即使后台线程忙着，用户也知道自己点到了。
        let label = crate::engine::job_label(&job, self.dry_run);
        self.push_console(Level::Info, format!("· 已提交：{label}"));
        if self.dry_run && job.touches_api() {
            self.push_console(
                Level::Warn,
                "演练模式开着：只会展示 prompt，不会真的调用模型（顶栏可取消勾选）",
            );
        }

        if runtime.submit(root, job, self.dry_run, self.credentials()) {
            self.pending_since = Some(Instant::now());
            self.pending_warned = false;
        } else {
            self.fail("后台线程已停止，请重启程序");
        }
    }

    /// 每帧调用：任务迟迟没开始就说出来，别让人对着不动的界面猜。
    pub fn watch_for_stalls(&mut self) {
        const PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);
        let stalled = self
            .pending_since
            .map(|at| at.elapsed() > PATIENCE)
            .unwrap_or(false);
        if stalled && !self.pending_warned {
            self.pending_warned = true;
            self.fail("任务提交后 3 秒仍未开始：后台可能被别的请求占住了，或线程已退出。可重启程序，并把「设置 → 关于与更新 → 自检」的内容发出来。");
        }
    }

    /// 把未保存的密钥与项目配置写下去。返回 false 表示保存失败，任务不该继续。
    fn flush_pending_edits(&mut self) -> bool {
        if self.keys_dirty {
            match self.settings.save() {
                Ok(()) => {
                    self.keys_dirty = false;
                    self.push_console(Level::Info, "已自动保存密钥");
                }
                Err(err) => {
                    self.fail(format!("保存密钥失败：{err:#}"));
                    return false;
                }
            }
        }
        if self.config_dirty {
            let Some(root) = self.root() else {
                return true;
            };
            self.config_draft.normalize();
            let result = crate::model::Project::open(&root).and_then(|mut project| {
                project.config = self.config_draft.clone();
                project.save_config()
            });
            match result {
                Ok(()) => {
                    self.config_dirty = false;
                    self.push_console(Level::Info, "已自动保存项目配置（服务商 / 模型改动）");
                }
                Err(err) => {
                    self.fail(format!("保存项目配置失败：{err:#}"));
                    return false;
                }
            }
        }
        true
    }

    /// Probe jobs are allowed without an open project.
    pub fn submit_probe(&mut self, runtime: &Runtime, job: Job) {
        if self.is_busy() {
            self.fail("已有任务在运行，请先等待或取消");
            return;
        }
        if !self.flush_pending_edits() {
            return;
        }
        let root = self
            .root()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        runtime.submit(root, job, false, self.credentials());
    }

    pub fn open_project(&mut self, runtime: &Runtime, path: &Path) {
        if !crate::model::Project::is_project(path) {
            self.fail(format!("不是 adrama 项目目录：{}", path.display()));
            return;
        }
        self.script_dirty = false;
        self.config_dirty = false;
        self.prompt_edit = None;
        self.selection.clear();
        self.live_status.clear();
        self.view = View::Dashboard;
        runtime.scan(path.to_path_buf());
    }

    pub fn refresh(&mut self, runtime: &Runtime) {
        if let Some(root) = self.root() {
            runtime.scan(root);
        }
    }

    /// Write the edited `project.toml`.
    pub fn save_project_config(&mut self, runtime: &Runtime) {
        let Some(root) = self.root() else {
            self.fail("尚未打开项目");
            return;
        };
        self.config_draft.normalize();
        let result = crate::model::Project::open(&root).and_then(|mut project| {
            project.config = self.config_draft.clone();
            project.save_config()
        });
        match result {
            Ok(()) => {
                self.config_dirty = false;
                self.note("项目配置已保存");
                self.refresh(runtime);
            }
            Err(err) => self.fail(format!("保存项目配置失败：{err:#}")),
        }
    }

    /// Write keys and UI preferences to the user config file.
    pub fn save_keys(&mut self) {
        match self.settings.save() {
            Ok(()) => {
                self.keys_dirty = false;
                self.note(format!(
                    "密钥已保存 → {}",
                    AppSettings::config_path().display()
                ));
            }
            Err(err) => self.fail(format!("保存密钥失败：{err:#}")),
        }
    }

    pub fn persist_ui_prefs(&mut self) {
        self.settings.ui.console_open = self.console_open;
        self.settings.ui.thumbnail_size = self.thumb_size;
        self.settings.ui.dry_run = self.dry_run;
        self.settings.ui.last_view = Some(self.view.key());
        let _ = self.settings.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AspectRatio, Breakdown, Project, Shot};
    use std::time::Duration;

    fn project(dir: &std::path::Path) -> Project {
        let project = Project::create(dir, "t", "s", AspectRatio::Landscape).unwrap();
        project.write_script("场景一\n内景 旧书店 傍晚").unwrap();
        project
            .save_breakdown(&Breakdown {
                title: "t".into(),
                shots: vec![Shot {
                    id: "shot_1".into(),
                    scene_id: "sc".into(),
                    number: 1,
                    framing: "中景".into(),
                    camera: "固定".into(),
                    visual: "画面".into(),
                    dialogue: String::new(),
                    sfx: String::new(),
                    duration_secs: 5,
                    character_ids: vec![],
                    prop_ids: vec![],
                    location_id: None,
                }],
                ..Default::default()
            })
            .unwrap();
        project
    }

    fn state_with(project: &Project) -> (AppState, Runtime) {
        let runtime = Runtime::spawn(eframe::egui::Context::default());
        let mut state = AppState::new(crate::settings::AppSettings::default());
        state.open_project(&runtime, &project.root);
        // 等第一次扫描回来，snapshot 就位后才谈得上提交任务
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while state.snapshot.is_none() && std::time::Instant::now() < deadline {
            while let Ok(update) = runtime.rx.try_recv() {
                state.apply(update);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        (state, runtime)
    }

    fn pump(state: &mut AppState, runtime: &Runtime, secs: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            while let Ok(update) = runtime.rx.try_recv() {
                state.apply(update);
            }
            if state.console.iter().any(|l| l.text.starts_with('✔') || l.text.starts_with('✖')) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 「点了运行没反应」的根因之一是任何一步失败都可能悄无声息。
    /// 提交后控制台必须立刻有记录，随后必须收到一条结束消息——成功或失败都行，
    /// 就是不能什么都没有。
    #[test]
    fn submitting_a_job_always_leaves_a_trace() {
        let tmp = tempfile::tempdir().unwrap();
        let project = project(&tmp.path().join("p"));
        let (mut state, runtime) = state_with(&project);
        assert!(state.snapshot.is_some(), "项目应当已加载");

        state.submit(&runtime, crate::engine::Job::Parse);
        assert!(
            state.console.iter().any(|l| l.text.contains("已提交")),
            "提交任务后控制台必须立刻留痕"
        );

        pump(&mut state, &runtime, 10);
        // 没配密钥，所以这里必然失败——重点是它必须“说话”
        assert!(
            state.console.iter().any(|l| l.text.starts_with('✖')),
            "任务结束必须有结论，控制台内容：{:?}",
            state.console.iter().map(|l| l.text.as_str()).collect::<Vec<_>>()
        );
        assert!(!state.is_busy(), "结束后不能一直停在「运行中」");
    }

    /// 演练模式下不该发出任何网络请求，但同样要有清楚的结论。
    #[test]
    fn dry_run_completes_without_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let project = project(&tmp.path().join("p"));
        let (mut state, runtime) = state_with(&project);
        state.dry_run = true;

        state.submit(&runtime, crate::engine::Job::Parse);
        pump(&mut state, &runtime, 10);

        assert!(
            state.console.iter().any(|l| l.text.contains("演练")),
            "演练模式必须明说自己没有调用模型：{:?}",
            state.console.iter().map(|l| l.text.as_str()).collect::<Vec<_>>()
        );
        assert!(!state.is_busy());
    }
}
