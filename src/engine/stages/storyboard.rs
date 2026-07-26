//! Stage 3 — one reference-anchored frame per shot.

use anyhow::{Context, Result};
use super::{references_for_shot, StageCtx, StageReport};
use crate::engine::prompts;
use crate::model::{ItemStatus, Project, Shot, Stage, StoryboardMeta};
use crate::providers::ImageRequest;

#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Empty means "every shot".
    pub shots: Vec<String>,
    /// Restrict to one scene number.
    pub scene: Option<u32>,
    pub force: bool,
    pub reset_prompts: bool,
}

impl Selection {
    pub fn only(shots: Vec<String>) -> Self {
        Self {
            shots,
            force: true,
            ..Default::default()
        }
    }
}

struct ShotJob {
    shot: Shot,
    prompt: String,
    references: Vec<std::path::PathBuf>,
    manual: bool,
    /// 本镜头的帧数覆盖（sidecar 里用户设的）。
    frames_override: Option<u32>,
    /// 成片顺序里的上一镜（链式衔接的来源）。
    prev_shot_id: Option<String>,
}

pub async fn run(ctx: &StageCtx<'_>, sel: &Selection) -> Result<StageReport> {
    let bd = ctx.project.load_breakdown()?;
    let style = &ctx.config().style;

    let jobs: Vec<ShotJob> = bd
        .shots
        .iter()
        .filter(|shot| sel.shots.is_empty() || sel.shots.iter().any(|s| s == &shot.id))
        .filter(|shot| match sel.scene {
            None => true,
            Some(n) => bd.scene(&shot.scene_id).map(|s| s.number) == Some(n),
        })
        .map(|shot| {
            let meta = stored_meta(ctx.project, &shot.id);
            let prev_shot_id = bd
                .shots
                .iter()
                .position(|s| s.id == shot.id)
                .and_then(|i| i.checked_sub(1))
                .map(|i| bd.shots[i].id.clone());
            ShotJob {
                prompt: stored_prompt(ctx.project, &shot.id, sel.reset_prompts)
                    .unwrap_or_else(|| prompts::storyboard_prompt(style, &bd, shot)),
                references: references_for_shot(ctx.project, &bd, shot),
                manual: meta.as_ref().map(|m| m.manual).unwrap_or(false),
                frames_override: meta.as_ref().and_then(|m| m.frames),
                prev_shot_id,
                shot: shot.clone(),
            }
        })
        .collect();

    if jobs.is_empty() {
        anyhow::bail!("没有匹配的镜头（检查镜头 id 或场次筛选）");
    }

    if ctx.dry_run {
        for job in &jobs {
            ctx.events.info(format!(
                "[演练] 分镜 {} → {}\n参考图 {} 张：{}\n{}",
                job.shot.id,
                ctx.project.storyboard_image(&job.shot.id).display(),
                job.references.len(),
                job.references
                    .iter()
                    .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
                    .collect::<Vec<_>>()
                    .join(", "),
                job.prompt
            ));
        }
        return Ok(StageReport {
            skipped: jobs.len(),
            ..Default::default()
        });
    }

    let provider = ctx.factory().image()?;
    ctx.events
        .info(format!("图像服务：{}", provider.endpoint()));
    if !provider.supports_references() {
        ctx.events
            .warn(prompts::no_reference_warning(&provider.endpoint().model));
    }

    let aspect = ctx.config().aspect;
    let global_frames = ctx.config().generation.frames_per_shot;
    ctx.events.info(format!(
        "为保证镜头衔接，分镜按顺序生成：下一镜首帧会参考上一镜末帧（每镜 {global_frames} 帧，可按镜头覆盖）"
    ));

    let bulk = sel.shots.is_empty();
    let total = jobs.len() as u32;
    let mut report = StageReport::default();
    // 顺序生成是有意的：第 i 帧要参考第 i-1 帧，下一镜首帧要参考上一镜末帧。
    // 并行会把这条链拆断。
    for (done, job) in jobs.iter().enumerate() {
        ctx.check_cancel()?;
        let id = job.shot.id.clone();
        ctx.events
            .progress(done as u32, total, format!("{}/{} · {id}", done + 1, total));

        if bulk && job.manual {
            report.skipped += 1;
            ctx.events
                .item(Stage::Storyboard, &id, ItemStatus::Done, "手动导入，已保留");
            continue;
        }

        let frames = job
            .frames_override
            .unwrap_or(global_frames)
            .clamp(2, 8);

        match generate_shot_frames(ctx, provider.as_ref(), &bd, job, frames, aspect, sel.force)
            .await
        {
            Ok(0) => {
                report.skipped += 1;
                ctx.events
                    .item(Stage::Storyboard, &id, ItemStatus::Done, "已存在，跳过");
            }
            Ok(n) => {
                report.generated += 1;
                ctx.events.item(
                    Stage::Storyboard,
                    &id,
                    ItemStatus::Done,
                    format!("已生成 {n} 帧"),
                );
                write_meta(ctx.project, job, ItemStatus::Done, None);
            }
            Err(err) => {
                let msg = format!("{err:#}");
                report.failed += 1;
                ctx.events.error(format!("{id}：{msg}"));
                ctx.events
                    .item(Stage::Storyboard, &id, ItemStatus::Failed, msg.clone());
                write_meta(ctx.project, job, ItemStatus::Failed, Some(msg));
            }
        }
    }
    ctx.check_cancel()?;
    Ok(report)
}

/// 生成一个镜头的全部关键帧，返回新生成的帧数。
///
/// 链式参考：第 1 帧带上一镜的末帧，第 i 帧带本镜的第 i-1 帧——
/// 视频用首末两帧约束后，片段之间才能自然衔接。
async fn generate_shot_frames(
    ctx: &StageCtx<'_>,
    provider: &dyn crate::providers::ImageProvider,
    bd: &crate::model::Breakdown,
    job: &ShotJob,
    frames: u32,
    aspect: crate::model::AspectRatio,
    force: bool,
) -> Result<u32> {
    let id = &job.shot.id;
    let mut generated = 0u32;

    for i in 1..=frames {
        ctx.check_cancel()?;
        let out = ctx.project.storyboard_keyframe(id, i);
        let exists = if i == 1 {
            // 旧命名 <id>.png 视作第 1 帧
            ctx.project.storyboard_image(id).is_file()
        } else {
            out.is_file()
        };
        if exists && !force {
            continue;
        }

        ctx.events.item(
            Stage::Storyboard,
            id,
            ItemStatus::Generating,
            format!("生成第 {i}/{frames} 帧"),
        );

        // 参考图：资产 + 链式帧
        let mut references = job.references.clone();
        let mut prompt = match i {
            1 => job.prompt.clone(),
            _ if i == frames => prompts::storyboard_last_prompt(&ctx.config().style, bd, &job.shot),
            _ => prompts::storyboard_middle_prompt(
                &ctx.config().style,
                bd,
                &job.shot,
                i,
                frames,
            ),
        };
        if i == 1 {
            if let Some(prev) = &job.prev_shot_id {
                if let Some(prev_last) = ctx.project.storyboard_last(prev) {
                    references.push(prev_last);
                    prompt.push_str(prompts::CHAIN_FROM_PREV);
                }
            }
        } else {
            let prev_frame = if i == 2 {
                ctx.project.storyboard_image(id)
            } else {
                ctx.project.storyboard_keyframe(id, i - 1)
            };
            if prev_frame.is_file() {
                references.push(prev_frame);
                prompt.push_str(prompts::CHAIN_SAME_SHOT);
            }
        }

        let bytes = provider
            .generate(ImageRequest {
                prompt: &prompt,
                aspect,
                references: &references,
            })
            .await
            .with_context(|| format!("分镜 {id} 第 {i} 帧"))?;

        tokio::fs::write(&out, &bytes)
            .await
            .with_context(|| format!("写入 {}", out.display()))?;
        ctx.events.artifact(&out);
        generated += 1;
    }
    Ok(generated)
}

fn stored_meta(project: &Project, shot_id: &str) -> Option<StoryboardMeta> {
    let text = std::fs::read_to_string(project.storyboard_meta(shot_id)).ok()?;
    serde_json::from_str(&text).ok()
}

fn stored_prompt(project: &Project, shot_id: &str, reset: bool) -> Option<String> {
    if reset {
        return None;
    }
    let meta = stored_meta(project, shot_id)?;
    (!meta.prompt.trim().is_empty()).then_some(meta.prompt)
}

fn write_meta(project: &Project, job: &ShotJob, status: ItemStatus, error: Option<String>) {
    let frames = project.storyboard_keyframes(&job.shot.id);
    let meta = StoryboardMeta {
        shot_id: job.shot.id.clone(),
        prompt: job.prompt.clone(),
        reference_assets: job
            .references
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        image: frames
            .first()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string()),
        last_image: (frames.len() >= 2)
            .then(|| frames.last().unwrap().file_name().unwrap().to_string_lossy().to_string()),
        frames: job.frames_override,
        status,
        error,
        manual: false,
    };
    if let Ok(text) = serde_json::to_string_pretty(&meta) {
        let _ = crate::model::project::write_atomic(
            &project.storyboard_meta(&job.shot.id),
            text.as_bytes(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::breakdown::Scene;
    use crate::model::{AspectRatio, Breakdown};

    fn shot(id: &str, scene: &str) -> Shot {
        Shot {
            id: id.into(),
            scene_id: scene.into(),
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
        }
    }

    #[test]
    fn stored_prompt_is_reused_and_resettable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();

        let meta = StoryboardMeta {
            shot_id: "s1".into(),
            prompt: "人工修改过的提示词".into(),
            ..Default::default()
        };
        std::fs::write(
            proj.storyboard_meta("s1"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();

        assert_eq!(
            stored_prompt(&proj, "s1", false).as_deref(),
            Some("人工修改过的提示词")
        );
        assert!(stored_prompt(&proj, "s1", true).is_none());
        assert!(stored_prompt(&proj, "missing", false).is_none());
    }

    #[test]
    fn scene_filter_selects_matching_shots() {
        let bd = Breakdown {
            scenes: vec![
                Scene {
                    id: "sc1".into(),
                    number: 1,
                    title: "a".into(),
                    description: String::new(),
                    location_id: None,
                    time_of_day: String::new(),
                },
                Scene {
                    id: "sc2".into(),
                    number: 2,
                    title: "b".into(),
                    description: String::new(),
                    location_id: None,
                    time_of_day: String::new(),
                },
            ],
            shots: vec![shot("a", "sc1"), shot("b", "sc2"), shot("c", "sc2")],
            ..Default::default()
        };

        let sel = Selection {
            scene: Some(2),
            ..Default::default()
        };
        let picked: Vec<&str> = bd
            .shots
            .iter()
            .filter(|s| match sel.scene {
                None => true,
                Some(n) => bd.scene(&s.scene_id).map(|sc| sc.number) == Some(n),
            })
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(picked, vec!["b", "c"]);
    }
}
