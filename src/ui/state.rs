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
    Stage(Stage),
    Settings,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Dashboard => "概览",
            View::Script => "剧本",
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
            View::Stage(stage) => format!("stage:{stage}"),
            View::Settings => "settings".into(),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "dashboard" => Some(View::Dashboard),
            "script" => Some(View::Script),
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
            View::Stage(Stage::Parse) => "模型拆出的角色、场次与镜头表",
            View::Stage(Stage::Assets) => "角色定妆照与服化道参考图",
            View::Stage(Stage::Storyboard) => "逐镜头画面，复用资产保持一致性",
            View::Stage(Stage::Video) => "分镜图生成视频片段并拼接成片",
            View::Settings => "服务商密钥、能力路由与项目参数",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Providers,
    Routing,
    Project,
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

    pub new_project: NewProjectForm,
    pub preview: Option<PathBuf>,
    pub dry_run: bool,
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
            settings_tab: SettingsTab::Providers,
            snapshot: None,
            busy: None,
            console: VecDeque::new(),
            console_open,
            banner: None,
            live_status: HashMap::new(),
            selection: HashMap::new(),
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
            new_project: NewProjectForm {
                parent: std::env::current_dir().ok(),
                name: "my-drama".into(),
                style: "cinematic, photorealistic, film grain".into(),
                aspect: crate::model::AspectRatio::Landscape,
            },
            preview: None,
            dry_run,
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
        }
    }

    fn apply_event(&mut self, event: StageEvent) {
        match event {
            StageEvent::Started { label } => {
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
        runtime.submit(root, job, self.dry_run, self.credentials());
    }

    /// Probe jobs are allowed without an open project.
    pub fn submit_probe(&mut self, runtime: &Runtime, job: Job) {
        if self.is_busy() {
            self.fail("已有任务在运行，请先等待或取消");
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
