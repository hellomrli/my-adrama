//! Bridge between the egui frame loop and the engine.
//!
//! 三条规则：UI 线程不做 IO，worker 不碰 egui 状态，一切以消息往来。
//!
//! 任务线程与「杂务」线程是分开的。更新检查要访问 GitHub，在部分网络下会一直
//! 挂到超时；如果和任务共用一个线程，用户点「运行」时请求只会静静排队，界面上
//! 什么都不会发生——这个坑踩过一次，别再合并回去。

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::engine::events::{CancelToken, EventSink, JobContext, StageEvent};
use crate::engine::{self, Job, JobOutcome, JobRequest};
use crate::model::{Breakdown, Project, ProjectConfig, ProjectIndex, ProjectState};
use crate::providers::Credentials;

/// 配音页的一行。
#[derive(Debug, Clone)]
pub struct VoiceItem {
    pub shot_id: String,
    pub dialogue: String,
    pub audio: Option<PathBuf>,
    pub manual: bool,
}

/// Everything a view needs to render a project, gathered off the UI thread.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub root: PathBuf,
    pub config: ProjectConfig,
    pub state: ProjectState,
    pub breakdown: Option<Breakdown>,
    pub index: ProjectIndex,
    pub script_path: Option<PathBuf>,
    pub script_text: String,
    pub breakdown_json: String,
    pub voice: Vec<VoiceItem>,
}

impl Snapshot {
    fn build(root: &std::path::Path) -> anyhow::Result<Self> {
        let project = Project::open(root)?;
        let breakdown = project.load_breakdown().ok();
        let mut index = ProjectIndex::build(&project, breakdown.as_ref());
        if let Some(bd) = &breakdown {
            engine::fill_default_prompts(&project.config, bd, &mut index);
        }
        let (script_path, script_text) = match project.read_script() {
            Ok((p, t)) => (Some(p), t),
            Err(_) => (None, String::new()),
        };
        let breakdown_json = std::fs::read_to_string(project.breakdown_path()).unwrap_or_default();

        let voice = breakdown
            .as_ref()
            .map(|bd| {
                bd.shots
                    .iter()
                    .filter(|s| !s.dialogue.trim().is_empty())
                    .map(|s| VoiceItem {
                        shot_id: s.id.clone(),
                        dialogue: s.dialogue.trim().to_string(),
                        audio: project.find_voice_clip(&s.id),
                        manual: std::fs::read_to_string(project.voice_meta(&s.id))
                            .ok()
                            .and_then(|raw| {
                                serde_json::from_str::<crate::model::VoiceMeta>(&raw).ok()
                            })
                            .map(|m| m.manual)
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            root: project.root.clone(),
            config: project.config,
            state: project.state,
            breakdown,
            index,
            script_path,
            script_text,
            breakdown_json,
            voice,
        })
    }
}

enum Request {
    Job(Box<JobRequest>),
    Scan(PathBuf),
    Shutdown,
}

/// 杂务：只在独立线程上跑，绝不挡住任务。
enum Chore {
    CheckUpdate,
    ApplyUpdate(Box<crate::update::ReleaseInfo>),
    InstallTool(crate::tools::Tool),
    Shutdown,
}

/// Messages flowing back to the UI thread.
pub enum Update {
    Event(StageEvent),
    Snapshot(Box<Snapshot>),
    ScanFailed(String),
    Outcome(Result<JobOutcome, String>),
    /// Result of an update check.
    NewVersion(Result<crate::update::UpdateStatus, String>),
    DownloadProgress { received: u64, total: u64 },
    Installed(Result<crate::update::Applied, String>),
    /// 工具（ffmpeg / Piper / 音色）安装进度与结果。
    ToolProgress {
        tool: crate::tools::Tool,
        received: u64,
        total: u64,
    },
    ToolInstalled(crate::tools::Tool, Result<String, String>),
}

pub struct Runtime {
    tx: Sender<Request>,
    chores: Sender<Chore>,
    pub rx: Receiver<Update>,
    cancel: CancelToken,
}

impl Runtime {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (up_tx, up_rx) = mpsc::channel::<Update>();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();

        let (chore_tx, chore_rx) = mpsc::channel::<Chore>();
        let chore_updates = up_tx.clone();
        let chore_ctx = ctx.clone();

        thread::Builder::new()
            .name("adrama-worker".into())
            .spawn(move || worker(req_rx, up_tx, ctx, worker_cancel))
            .expect("spawn worker thread");

        thread::Builder::new()
            .name("adrama-chores".into())
            .spawn(move || chore_worker(chore_rx, chore_updates, chore_ctx))
            .expect("spawn chore thread");

        Self {
            tx: req_tx,
            chores: chore_tx,
            rx: up_rx,
            cancel,
        }
    }

    /// 返回 false 表示后台线程已经不在了（此时界面必须给出反馈，而不是静默）。
    pub fn submit(&self, root: PathBuf, job: Job, dry_run: bool, credentials: Credentials) -> bool {
        self.cancel.reset();
        self.tx
            .send(Request::Job(Box::new(JobRequest {
                root,
                job,
                dry_run,
                credentials,
            })))
            .is_ok()
    }

    /// Ask the worker to re-read the project from disk.
    pub fn scan(&self, root: PathBuf) {
        let _ = self.tx.send(Request::Scan(root));
    }

    /// Ask GitHub whether a newer release exists. 跑在杂务线程上，不影响任务。
    pub fn check_update(&self) {
        let _ = self.chores.send(Chore::CheckUpdate);
    }

    /// Download and install the given release.
    pub fn apply_update(&self, release: crate::update::ReleaseInfo) {
        let _ = self.chores.send(Chore::ApplyUpdate(Box::new(release)));
    }

    /// 下载并安装本地工具（ffmpeg / Piper / 音色）。
    pub fn install_tool(&self, tool: crate::tools::Tool) {
        let _ = self.chores.send(Chore::InstallTool(tool));
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.tx.send(Request::Shutdown);
        let _ = self.chores.send(Chore::Shutdown);
    }
}

/// Forwards engine events to the UI and wakes the frame loop.
struct UiSink {
    tx: Sender<Update>,
    ctx: egui::Context,
}

impl EventSink for UiSink {
    fn emit(&self, event: StageEvent) {
        let _ = self.tx.send(Update::Event(event));
        self.ctx.request_repaint();
    }
}

fn worker(
    requests: Receiver<Request>,
    updates: Sender<Update>,
    ctx: egui::Context,
    cancel: CancelToken,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(err) => {
            let _ = updates.send(Update::Outcome(Err(format!("异步运行时启动失败：{err}"))));
            ctx.request_repaint();
            return;
        }
    };

    let sink = Arc::new(UiSink {
        tx: updates.clone(),
        ctx: ctx.clone(),
    });

    while let Ok(request) = requests.recv() {
        match request {
            Request::Shutdown => break,
            Request::Scan(root) => {
                send_snapshot(&updates, &ctx, &root);
            }
            Request::Job(req) => {
                let root = req.root.clone();
                let started = Instant::now();
                cancel.reset();
                let job_ctx = JobContext::new(sink.clone(), cancel.clone());

                // 任何 panic 都必须变成一条 Finished，否则界面会永远停在「运行中」，
                // 之后每次点击都只会提示「已有任务在运行」。
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    rt.block_on(engine::run_job(*req, &job_ctx))
                }))
                .unwrap_or_else(|payload| {
                    let detail = panic_message(&payload);
                    let _ = updates.send(Update::Event(StageEvent::Finished {
                        ok: false,
                        message: format!("内部错误（已中止）：{detail}"),
                    }));
                    Err(anyhow::anyhow!("内部错误：{detail}"))
                });

                let outcome = result.map_err(|e| format!("{e:#}"));
                let _ = updates.send(Update::Outcome(outcome));
                tracing::debug!("job finished in {:?}", started.elapsed());

                // The project changed on disk; refresh before the UI redraws.
                send_snapshot(&updates, &ctx, &root);
            }
        }
    }
}

/// 更新检查 / 下载：独立线程，慢或超时都只影响它自己。
fn chore_worker(chores: Receiver<Chore>, updates: Sender<Update>, ctx: egui::Context) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            tracing::warn!("更新线程启动失败：{err}");
            return;
        }
    };

    while let Ok(chore) = chores.recv() {
        match chore {
            Chore::Shutdown => break,
            Chore::CheckUpdate => {
                let result = rt
                    .block_on(crate::update::check())
                    .map_err(|e| format!("{e:#}"));
                let _ = updates.send(Update::NewVersion(result));
                ctx.request_repaint();
            }
            Chore::InstallTool(tool) => {
                let progress_tx = updates.clone();
                let progress_ctx = ctx.clone();
                let result = rt
                    .block_on(crate::tools::install(tool, |received, total| {
                        let _ = progress_tx.send(Update::ToolProgress {
                            tool,
                            received,
                            total,
                        });
                        progress_ctx.request_repaint();
                    }))
                    .map_err(|e| format!("{e:#}"));
                let _ = updates.send(Update::ToolInstalled(tool, result));
                ctx.request_repaint();
            }
            Chore::ApplyUpdate(release) => {
                let progress_tx = updates.clone();
                let progress_ctx = ctx.clone();
                let result = rt
                    .block_on(crate::update::download_and_apply(
                        &release,
                        |received, total| {
                            let _ = progress_tx.send(Update::DownloadProgress { received, total });
                            progress_ctx.request_repaint();
                        },
                    ))
                    .map_err(|e| format!("{e:#}"));
                let _ = updates.send(Update::Installed(result));
                ctx.request_repaint();
            }
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知原因".into()
    }
}

fn send_snapshot(updates: &Sender<Update>, ctx: &egui::Context, root: &std::path::Path) {
    let update = match Snapshot::build(root) {
        Ok(snapshot) => Update::Snapshot(Box::new(snapshot)),
        Err(err) => Update::ScanFailed(format!("{err:#}")),
    };
    let _ = updates.send(update);
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Job;
    use crate::model::{AspectRatio, Breakdown, Project, Shot, Stage};
    use std::time::Duration;

    fn demo_project(dir: &std::path::Path) -> Project {
        let project = Project::create(dir, "t", "s", AspectRatio::Landscape).unwrap();
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
                    visual_end: String::new(),
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

    /// 回归：更新检查曾经和任务共用一个线程。GitHub 连不上时（国内很常见）
    /// 用户点「运行」只会静静排队，界面上毫无反应。任务必须不受杂务影响。
    #[test]
    fn a_hanging_update_check_does_not_block_jobs() {
        // 指向一个不可路由的地址，让更新检查卡在连接阶段（约 8 秒）。
        std::env::set_var("HTTPS_PROXY", "http://10.255.255.1:9999");

        let tmp = tempfile::tempdir().unwrap();
        let project = demo_project(&tmp.path().join("p"));

        let runtime = Runtime::spawn(egui::Context::default());
        runtime.check_update();

        let submitted = runtime.submit(
            project.root.clone(),
            Job::Approve(Stage::Parse),
            false,
            crate::providers::Credentials::default(),
        );
        assert!(submitted, "任务应当被后台线程接收");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut finished = false;
        while std::time::Instant::now() < deadline && !finished {
            while let Ok(update) = runtime.rx.try_recv() {
                if let Update::Event(StageEvent::Finished { ok, .. }) = update {
                    assert!(ok, "审核任务应当成功");
                    finished = true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        std::env::remove_var("HTTPS_PROXY");
        assert!(
            finished,
            "更新检查卡住时任务仍然必须跑起来（否则又回到「点了没反应」）"
        );
    }
}
