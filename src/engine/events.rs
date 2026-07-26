//! Progress reporting and cancellation.
//!
//! Stages used to `println!` into a terminal nobody was watching (the GUI ran
//! them on a worker thread and showed a spinner). They now emit structured
//! events that both front-ends render in their own way.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::model::{ItemStatus, Stage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub enum StageEvent {
    /// A job started; `label` is user-facing ("生成资产（演练）").
    Started { label: String },
    Log { level: Level, message: String },
    /// Overall progress within the current job.
    Progress {
        done: u32,
        total: u32,
        detail: String,
    },
    /// One work item changed state.
    Item {
        stage: Stage,
        id: String,
        status: ItemStatus,
        detail: String,
    },
    /// A file was written — front-ends can refresh previews.
    Artifact { path: std::path::PathBuf },
    Finished { ok: bool, message: String },
}

/// Where events go. Implemented by the CLI printer and the GUI channel.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: StageEvent);
}

/// Drops everything — used by tests.
#[cfg(test)]
pub struct NullSink;

#[cfg(test)]
impl EventSink for NullSink {
    fn emit(&self, _event: StageEvent) {}
}

impl EventSink for std::sync::mpsc::Sender<StageEvent> {
    fn emit(&self, event: StageEvent) {
        let _ = self.send(event);
    }
}

/// Cooperative cancellation, checked between items *and* inside the video
/// polling loop, so "取消" is not a lie during a 30-minute Veo wait.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            anyhow::bail!("任务已取消");
        }
        Ok(())
    }
}

/// Handle passed down through every stage.
#[derive(Clone)]
pub struct JobContext {
    pub sink: Arc<dyn EventSink>,
    pub cancel: CancelToken,
}

impl JobContext {
    pub fn new(sink: Arc<dyn EventSink>, cancel: CancelToken) -> Self {
        Self { sink, cancel }
    }

    #[cfg(test)]
    pub fn null() -> Self {
        Self::new(Arc::new(NullSink), CancelToken::new())
    }

    pub fn info(&self, message: impl Into<String>) {
        self.sink.emit(StageEvent::Log {
            level: Level::Info,
            message: message.into(),
        });
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.sink.emit(StageEvent::Log {
            level: Level::Warn,
            message: message.into(),
        });
    }

    pub fn error(&self, message: impl Into<String>) {
        self.sink.emit(StageEvent::Log {
            level: Level::Error,
            message: message.into(),
        });
    }

    pub fn progress(&self, done: u32, total: u32, detail: impl Into<String>) {
        self.sink.emit(StageEvent::Progress {
            done,
            total,
            detail: detail.into(),
        });
    }

    pub fn item(
        &self,
        stage: Stage,
        id: impl Into<String>,
        status: ItemStatus,
        detail: impl Into<String>,
    ) {
        self.sink.emit(StageEvent::Item {
            stage,
            id: id.into(),
            status,
            detail: detail.into(),
        });
    }

    pub fn artifact(&self, path: impl Into<std::path::PathBuf>) {
        self.sink.emit(StageEvent::Artifact { path: path.into() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Collect(Mutex<Vec<StageEvent>>);

    impl EventSink for Collect {
        fn emit(&self, event: StageEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn cancel_token_is_shared() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(token.check().is_ok());
        clone.cancel();
        assert!(token.is_cancelled());
        assert!(token.check().is_err());
        token.reset();
        assert!(clone.check().is_ok());
    }

    #[test]
    fn context_forwards_events() {
        let sink = Arc::new(Collect::default());
        let ctx = JobContext::new(sink.clone(), CancelToken::new());
        ctx.info("hello");
        ctx.progress(1, 3, "工作中");
        assert_eq!(sink.0.lock().unwrap().len(), 2);
    }
}
