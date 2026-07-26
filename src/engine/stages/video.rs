//! Stage 4 — image-to-video. The expensive one, so it resumes interrupted
//! operations, polls with cancellation, and defaults to one job at a time.

use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{run_items, ItemOutcome, StageCtx, StageReport};
use crate::engine::prompts;
use crate::model::{ItemStatus, Project, Shot, Stage, VideoMeta};
use crate::providers::{VideoPoll, VideoProvider, VideoRequest};

#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Empty means "every shot".
    pub shots: Vec<String>,
    pub force: bool,
}

impl Selection {
    pub fn only(shots: Vec<String>) -> Self {
        Self {
            shots,
            force: true,
        }
    }
}

pub async fn run(ctx: &StageCtx<'_>, sel: &Selection) -> Result<StageReport> {
    let bd = ctx.project.load_breakdown()?;
    let shots: Vec<Shot> = bd
        .shots
        .iter()
        .filter(|s| sel.shots.is_empty() || sel.shots.iter().any(|want| want == &s.id))
        .cloned()
        .collect();

    if shots.is_empty() {
        bail!("没有匹配的镜头（检查镜头 id）");
    }

    if ctx.dry_run {
        for shot in &shots {
            ctx.events.info(format!(
                "[演练] 视频 {} · {}s · {}\n首帧 {}\n{}",
                shot.id,
                shot.duration_secs,
                prompts::size_hint(ctx.config().aspect),
                ctx.project.storyboard_image(&shot.id).display(),
                prompts::video_prompt(shot)
            ));
        }
        return Ok(StageReport {
            skipped: shots.len(),
            ..Default::default()
        });
    }

    let provider = ctx.factory().video()?;
    ctx.events
        .info(format!("视频服务：{}", provider.endpoint()));

    let gen = ctx.config().generation.clone();
    let aspect = ctx.config().aspect;
    let items: Vec<(String, Shot)> = shots.into_iter().map(|s| (s.id.clone(), s)).collect();

    run_items(
        ctx.events,
        Stage::Video,
        gen.video_concurrency,
        items,
        |id, shot| {
            let provider = Arc::clone(&provider);
            let gen = gen.clone();
            async move {
                ctx.check_cancel()?;
                if sel.shots.is_empty()
                    && read_meta(ctx.project, &shot.id)
                        .map(|m| m.manual)
                        .unwrap_or(false)
                {
                    return Ok(ItemOutcome::Skipped("手动导入，已保留".into()));
                }
                match generate_clip(ctx, provider.as_ref(), &shot, aspect, &gen, sel.force).await {
                    Ok(outcome) => Ok(outcome),
                    Err(err) => {
                        let msg = format!("{err:#}");
                        // Keep the operation id so a later run can resume instead
                        // of paying for the clip twice.
                        if let Some(mut meta) = read_meta(ctx.project, &id) {
                            meta.status = ItemStatus::Failed;
                            meta.error = Some(msg.clone());
                            write_meta(ctx.project, &meta);
                        }
                        Ok(ItemOutcome::Failed(msg))
                    }
                }
            }
        },
    )
    .await
}

async fn generate_clip(
    ctx: &StageCtx<'_>,
    provider: &dyn VideoProvider,
    shot: &Shot,
    aspect: crate::model::AspectRatio,
    gen: &crate::model::GenerationSettings,
    force: bool,
) -> Result<ItemOutcome> {
    let clip = ctx.project.video_clip(&shot.id);
    let frame = ctx.project.storyboard_image(&shot.id);

    if clip.is_file() && !force {
        return Ok(ItemOutcome::Skipped("已存在，跳过".into()));
    }
    if !frame.is_file() {
        bail!("缺少分镜首帧：{}", frame.display());
    }

    let existing = read_meta(ctx.project, &shot.id);
    let prompt = existing
        .as_ref()
        .map(|m| m.prompt.clone())
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| prompts::video_prompt(shot));

    // Resume a job that was already paid for.
    let resumed = existing
        .as_ref()
        .filter(|_| !force)
        .and_then(|m| m.operation_name.clone());

    let operation = match resumed {
        Some(op) => {
            ctx.events
                .info(format!("{}：恢复已提交的任务 {op}", shot.id));
            op
        }
        None => {
            ctx.events
                .item(Stage::Video, &shot.id, ItemStatus::Generating, "提交任务");
            let op = provider
                .submit(VideoRequest {
                    prompt: &prompt,
                    image: &frame,
                    aspect,
                    duration_secs: shot.duration_secs.clamp(2, gen.max_shot_seconds),
                })
                .await
                .with_context(|| format!("提交视频任务 {}", shot.id))?;

            write_meta(
                ctx.project,
                &VideoMeta {
                    shot_id: shot.id.clone(),
                    prompt: prompt.clone(),
                    source_image: Some(frame.display().to_string()),
                    operation_name: Some(op.clone()),
                    video: None,
                    status: ItemStatus::Generating,
                    error: None,
                    manual: false,
                },
            );
            op
        }
    };

    let bytes = wait_for_clip(ctx, provider, &shot.id, &operation, gen).await?;
    tokio::fs::write(&clip, &bytes)
        .await
        .with_context(|| format!("写入 {}", clip.display()))?;
    ctx.events.artifact(&clip);

    write_meta(
        ctx.project,
        &VideoMeta {
            shot_id: shot.id.clone(),
            prompt,
            source_image: Some(frame.display().to_string()),
            operation_name: Some(operation),
            video: Some(format!("{}.mp4", shot.id)),
            status: ItemStatus::Done,
            error: None,
            manual: false,
        },
    );
    Ok(ItemOutcome::Generated)
}

/// Poll until the operation finishes, surfacing elapsed time and honouring
/// cancellation — a 30-minute wait used to ignore the cancel button entirely.
async fn wait_for_clip(
    ctx: &StageCtx<'_>,
    provider: &dyn VideoProvider,
    shot_id: &str,
    operation: &str,
    gen: &crate::model::GenerationSettings,
) -> Result<Vec<u8>> {
    let started = Instant::now();
    let timeout = Duration::from_secs(gen.video_timeout_secs);
    let interval = Duration::from_secs(gen.video_poll_secs);

    loop {
        ctx.check_cancel()?;
        if started.elapsed() > timeout {
            bail!(
                "视频任务超时（{} 秒），操作 id {operation} 已保存，可稍后重跑以继续等待",
                timeout.as_secs()
            );
        }

        match provider.poll(operation).await? {
            VideoPoll::Ready(bytes) => return Ok(bytes),
            VideoPoll::Pending => {
                let secs = started.elapsed().as_secs();
                ctx.events.item(
                    Stage::Video,
                    shot_id,
                    ItemStatus::Generating,
                    format!("渲染中 · 已等待 {secs}s"),
                );
                sleep_cancellable(ctx, interval).await?;
            }
        }
    }
}

/// Sleep in slices so cancellation lands within ~200ms.
async fn sleep_cancellable(ctx: &StageCtx<'_>, total: Duration) -> Result<()> {
    let slice = Duration::from_millis(200);
    let mut waited = Duration::ZERO;
    while waited < total {
        ctx.check_cancel()?;
        tokio::time::sleep(slice.min(total - waited)).await;
        waited += slice;
    }
    Ok(())
}

fn read_meta(project: &Project, shot_id: &str) -> Option<VideoMeta> {
    let text = std::fs::read_to_string(project.video_meta(shot_id)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_meta(project: &Project, meta: &VideoMeta) {
    if let Ok(text) = serde_json::to_string_pretty(meta) {
        let _ = crate::model::project::write_atomic(
            &project.video_meta(&meta.shot_id),
            text.as_bytes(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AspectRatio;

    #[test]
    fn meta_round_trips_and_keeps_operation_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();

        write_meta(
            &proj,
            &VideoMeta {
                shot_id: "s1".into(),
                prompt: "p".into(),
                operation_name: Some("operations/abc".into()),
                status: ItemStatus::Generating,
                ..Default::default()
            },
        );

        let back = read_meta(&proj, "s1").expect("meta readable");
        assert_eq!(back.operation_name.as_deref(), Some("operations/abc"));
        assert_eq!(back.status, ItemStatus::Generating);
    }

    #[tokio::test]
    async fn sleep_is_interrupted_by_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();
        let creds = crate::providers::Credentials::default();
        let events = crate::engine::events::JobContext::null();
        let ctx = StageCtx {
            project: &proj,
            credentials: &creds,
            events: &events,
            dry_run: false,
        };

        events.cancel.cancel();
        let started = Instant::now();
        let err = sleep_cancellable(&ctx, Duration::from_secs(30))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("取消"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
