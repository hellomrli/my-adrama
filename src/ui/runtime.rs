//! Bridge between the egui frame loop and the engine.
//!
//! Two rules: the UI thread never blocks on IO or HTTP, and the worker never
//! touches egui state. Everything crosses as messages, and the worker nudges
//! egui to repaint so the UI is event-driven rather than polling.

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

        Ok(Self {
            root: project.root.clone(),
            config: project.config,
            state: project.state,
            breakdown,
            index,
            script_path,
            script_text,
            breakdown_json,
        })
    }
}

enum Request {
    Job(Box<JobRequest>),
    Scan(PathBuf),
    Shutdown,
}

/// Messages flowing back to the UI thread.
pub enum Update {
    Event(StageEvent),
    Snapshot(Box<Snapshot>),
    ScanFailed(String),
    Outcome(Result<JobOutcome, String>),
}

pub struct Runtime {
    tx: Sender<Request>,
    pub rx: Receiver<Update>,
    cancel: CancelToken,
}

impl Runtime {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (up_tx, up_rx) = mpsc::channel::<Update>();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();

        thread::Builder::new()
            .name("adrama-worker".into())
            .spawn(move || worker(req_rx, up_tx, ctx, worker_cancel))
            .expect("spawn worker thread");

        Self {
            tx: req_tx,
            rx: up_rx,
            cancel,
        }
    }

    pub fn submit(&self, root: PathBuf, job: Job, dry_run: bool, credentials: Credentials) {
        self.cancel.reset();
        let _ = self.tx.send(Request::Job(Box::new(JobRequest {
            root,
            job,
            dry_run,
            credentials,
        })));
    }

    /// Ask the worker to re-read the project from disk.
    pub fn scan(&self, root: PathBuf) {
        let _ = self.tx.send(Request::Scan(root));
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.tx.send(Request::Shutdown);
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
                let result = rt.block_on(engine::run_job(*req, &job_ctx));

                let outcome = result.map_err(|e| format!("{e:#}"));
                let _ = updates.send(Update::Outcome(outcome));
                tracing::debug!("job finished in {:?}", started.elapsed());

                // The project changed on disk; refresh before the UI redraws.
                send_snapshot(&updates, &ctx, &root);
            }
        }
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
