//! Stage runners. Each one takes a [`StageCtx`], reports through events, and
//! respects cancellation between items.

pub mod assets;
pub mod export;
pub mod voice;
pub mod parse;
pub mod storyboard;
pub mod video;

use anyhow::Result;
use futures::future::LocalBoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::future::Future;
use std::path::PathBuf;

use crate::engine::events::JobContext;
use crate::model::{
    AssetKind, Breakdown, ItemStatus, Project, ProjectConfig, Shot, Stage,
};
use crate::providers::{Credentials, ProviderFactory};

/// Everything a stage needs, and nothing else.
pub struct StageCtx<'a> {
    pub project: &'a Project,
    pub credentials: &'a Credentials,
    pub events: &'a JobContext,
    /// Assemble and show prompts, but never call a paid API.
    pub dry_run: bool,
}

impl<'a> StageCtx<'a> {
    pub fn config(&self) -> &ProjectConfig {
        &self.project.config
    }

    pub fn factory(&self) -> ProviderFactory<'_> {
        ProviderFactory::new(&self.project.config, self.credentials)
    }

    /// Bail out if the user pressed cancel.
    pub fn check_cancel(&self) -> Result<()> {
        self.events.cancel.check()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageReport {
    pub generated: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl StageReport {
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("生成 {}", self.generated)];
        if self.skipped > 0 {
            parts.push(format!("跳过 {}", self.skipped));
        }
        if self.failed > 0 {
            parts.push(format!("失败 {}", self.failed));
        }
        parts.join(" · ")
    }
}

pub enum ItemOutcome {
    Generated,
    /// Already on disk, or filtered out.
    Skipped(String),
    Failed(String),
}

/// Run per-item work with bounded concurrency, emitting progress as each item
/// settles and stopping promptly when cancelled.
pub async fn run_items<T, F, Fut>(
    ctx: &JobContext,
    stage: Stage,
    concurrency: usize,
    items: Vec<(String, T)>,
    run: F,
) -> Result<StageReport>
where
    F: Fn(String, T) -> Fut,
    Fut: Future<Output = Result<ItemOutcome>>,
{
    let total = items.len() as u32;
    let mut report = StageReport::default();
    if total == 0 {
        return Ok(report);
    }

    let concurrency = concurrency.max(1);
    let mut queue = items.into_iter();
    let mut running = FuturesUnordered::new();
    let mut done = 0u32;

    // Boxed so both spawn sites push the same future type.
    let spawn = |id: String, payload: T| -> LocalBoxFuture<'_, (String, Result<ItemOutcome>)> {
        let fut = run(id.clone(), payload);
        Box::pin(async move { (id, fut.await) })
    };

    for _ in 0..concurrency {
        let Some((id, payload)) = queue.next() else {
            break;
        };
        running.push(spawn(id, payload));
    }

    while let Some((id, result)) = running.next().await {
        done += 1;
        match result {
            Ok(ItemOutcome::Generated) => {
                report.generated += 1;
                ctx.item(stage, &id, ItemStatus::Done, "已生成");
            }
            Ok(ItemOutcome::Skipped(reason)) => {
                report.skipped += 1;
                ctx.item(stage, &id, ItemStatus::Done, reason);
            }
            Ok(ItemOutcome::Failed(err)) => {
                report.failed += 1;
                ctx.error(format!("{id}：{err}"));
                ctx.item(stage, &id, ItemStatus::Failed, err);
            }
            Err(err) => {
                report.failed += 1;
                let msg = format!("{err:#}");
                ctx.error(format!("{id}：{msg}"));
                ctx.item(stage, &id, ItemStatus::Failed, msg);
            }
        }
        ctx.progress(done, total, format!("{done}/{total} · {id}"));

        if ctx.cancel.is_cancelled() {
            break;
        }
        if let Some((next_id, payload)) = queue.next() {
            running.push(spawn(next_id, payload));
        }
    }

    ctx.cancel.check()?;
    Ok(report)
}

/// Reference images that anchor a storyboard frame: every character in the
/// shot, the location, then any props.
pub fn references_for_shot(project: &Project, bd: &Breakdown, shot: &Shot) -> Vec<PathBuf> {
    let mut refs = Vec::new();

    for cid in &shot.character_ids {
        let dir = project.asset_dir(AssetKind::Character, cid);
        let front = dir.join("front.png");
        if front.is_file() {
            refs.push(front);
        } else if let Some(any) = first_image(&dir) {
            refs.push(any);
        }
    }

    if let Some(loc) = bd.location_for_shot(shot) {
        let p = project.asset_dir(AssetKind::Location, &loc.id).join("ref.png");
        if p.is_file() {
            refs.push(p);
        }
    }

    for pid in &shot.prop_ids {
        let p = project.asset_dir(AssetKind::Prop, pid).join("ref.png");
        if p.is_file() {
            refs.push(p);
        }
    }

    refs
}

fn first_image(dir: &std::path::Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| crate::model::project::has_extension(p, crate::model::index::IMAGE_EXTENSIONS))
        .collect();
    files.sort();
    files.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::events::CancelToken;
    use crate::model::AspectRatio;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn items_run_and_are_counted() {
        let ctx = JobContext::null();
        let items: Vec<(String, u32)> = (0..5).map(|i| (format!("item_{i}"), i)).collect();
        let report = run_items(&ctx, Stage::Assets, 2, items, |_id, n| async move {
            Ok(if n % 2 == 0 {
                ItemOutcome::Generated
            } else {
                ItemOutcome::Skipped("已存在".into())
            })
        })
        .await
        .unwrap();

        assert_eq!(report.generated, 3);
        assert_eq!(report.skipped, 2);
        assert_eq!(report.generated + report.skipped, 5);
    }

    #[tokio::test]
    async fn failures_do_not_abort_the_batch() {
        let ctx = JobContext::null();
        let items: Vec<(String, u32)> = (0..4).map(|i| (format!("i{i}"), i)).collect();
        let report = run_items(&ctx, Stage::Assets, 1, items, |_id, n| async move {
            if n == 1 {
                anyhow::bail!("boom");
            }
            Ok(ItemOutcome::Generated)
        })
        .await
        .unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.generated, 3);
    }

    #[tokio::test]
    async fn cancellation_stops_starting_new_items() {
        let cancel = CancelToken::new();
        let ctx = JobContext::new(std::sync::Arc::new(crate::engine::events::NullSink), cancel.clone());
        let started = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = started.clone();
        let items: Vec<(String, u32)> = (0..20).map(|i| (format!("i{i}"), i)).collect();

        let err = run_items(&ctx, Stage::Assets, 1, items, move |_id, _n| {
            let counter = counter.clone();
            let cancel = cancel.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                cancel.cancel();
                Ok(ItemOutcome::Generated)
            }
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("取消"));
        assert!(started.load(Ordering::SeqCst) < 20);
    }

    #[test]
    fn references_prefer_front_view_and_include_location() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();

        let char_dir = proj.asset_dir(AssetKind::Character, "char_a");
        std::fs::create_dir_all(&char_dir).unwrap();
        std::fs::write(char_dir.join("side.png"), b"x").unwrap();
        std::fs::write(char_dir.join("front.png"), b"x").unwrap();

        let loc_dir = proj.asset_dir(AssetKind::Location, "loc_1");
        std::fs::create_dir_all(&loc_dir).unwrap();
        std::fs::write(loc_dir.join("ref.png"), b"x").unwrap();

        let bd = Breakdown {
            locations: vec![crate::model::Location {
                id: "loc_1".into(),
                name: "仓库".into(),
                description: "d".into(),
                time_of_day: String::new(),
            }],
            ..Default::default()
        };
        let shot = Shot {
            id: "s1".into(),
            scene_id: "sc".into(),
            number: 1,
            framing: "wide".into(),
            camera: "static".into(),
            visual: "v".into(),
            visual_end: String::new(),
            dialogue: String::new(),
            sfx: String::new(),
            duration_secs: 5,
            character_ids: vec!["char_a".into()],
            prop_ids: vec![],
            location_id: Some("loc_1".into()),
        };

        let refs = references_for_shot(&proj, &bd, &shot);
        assert_eq!(refs.len(), 2);
        assert!(refs[0].ends_with("front.png"));
        assert!(refs[1].ends_with("ref.png"));
    }
}
