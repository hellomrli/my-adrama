//! Stage 3 — one reference-anchored frame per shot.

use anyhow::{Context, Result};
use std::sync::Arc;

use super::{references_for_shot, run_items, ItemOutcome, StageCtx, StageReport};
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
        .map(|shot| ShotJob {
            shot: shot.clone(),
            prompt: stored_prompt(ctx.project, &shot.id, sel.reset_prompts)
                .unwrap_or_else(|| prompts::storyboard_prompt(style, &bd, shot)),
            references: references_for_shot(ctx.project, &bd, shot),
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
    let concurrency = ctx.config().generation.image_concurrency;
    let items: Vec<(String, ShotJob)> = jobs.into_iter().map(|j| (j.shot.id.clone(), j)).collect();

    run_items(ctx.events, Stage::Storyboard, concurrency, items, |id, job| {
        let provider = Arc::clone(&provider);
        async move {
            ctx.check_cancel()?;
            let out = ctx.project.storyboard_image(&id);
            if out.is_file() && !sel.force {
                return Ok(ItemOutcome::Skipped("已存在，跳过".into()));
            }

            ctx.events
                .item(Stage::Storyboard, &id, ItemStatus::Generating, "生成中");

            let result = provider
                .generate(ImageRequest {
                    prompt: &job.prompt,
                    aspect,
                    references: &job.references,
                })
                .await;

            match result {
                Ok(bytes) => {
                    tokio::fs::write(&out, &bytes)
                        .await
                        .with_context(|| format!("写入 {}", out.display()))?;
                    ctx.events.artifact(&out);
                    write_meta(ctx.project, &job, ItemStatus::Done, None);
                    Ok(ItemOutcome::Generated)
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    write_meta(ctx.project, &job, ItemStatus::Failed, Some(msg.clone()));
                    Ok(ItemOutcome::Failed(msg))
                }
            }
        }
    })
    .await
}

fn stored_prompt(project: &Project, shot_id: &str, reset: bool) -> Option<String> {
    if reset {
        return None;
    }
    let text = std::fs::read_to_string(project.storyboard_meta(shot_id)).ok()?;
    let meta: StoryboardMeta = serde_json::from_str(&text).ok()?;
    (!meta.prompt.trim().is_empty()).then_some(meta.prompt)
}

fn write_meta(project: &Project, job: &ShotJob, status: ItemStatus, error: Option<String>) {
    let image = project.storyboard_image(&job.shot.id);
    let meta = StoryboardMeta {
        shot_id: job.shot.id.clone(),
        prompt: job.prompt.clone(),
        reference_assets: job
            .references
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        image: image
            .is_file()
            .then(|| image.file_name().unwrap().to_string_lossy().to_string()),
        status,
        error,
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
