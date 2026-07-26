//! Stage 2 — character sheets, costumes, props and locations.

use anyhow::{Context, Result};
use std::sync::Arc;

use super::{run_items, ItemOutcome, StageCtx, StageReport};
use crate::engine::prompts;
use crate::model::{AssetKind, AssetMeta, Breakdown, ItemStatus, Project, Stage};
use crate::providers::{ImageProvider, ImageRequest};

/// Which assets to (re)generate.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Empty means "everything".
    pub ids: Vec<String>,
    /// Overwrite existing images instead of skipping them.
    pub force: bool,
    /// Throw away the stored prompt and recompose it from the breakdown.
    pub reset_prompts: bool,
}

impl Selection {
    pub fn only(ids: Vec<String>) -> Self {
        Self {
            ids,
            force: true,
            reset_prompts: false,
        }
    }

    fn matches(&self, id: &str, name: &str) -> bool {
        self.ids.is_empty() || self.ids.iter().any(|want| want == id || want == name)
    }
}

struct AssetJob {
    kind: AssetKind,
    id: String,
    name: String,
    /// Base prompt before per-view suffixes.
    prompt: String,
    /// Output file names; characters get several views.
    views: Vec<(String, String)>,
}

pub async fn run(ctx: &StageCtx<'_>, sel: &Selection) -> Result<StageReport> {
    let bd = ctx.project.load_breakdown()?;
    let jobs = build_jobs(ctx.project, &bd, sel);

    if jobs.is_empty() {
        anyhow::bail!(
            "没有匹配的资产{}",
            if sel.ids.is_empty() {
                "（breakdown 中没有角色/服装/道具/场景）".to_string()
            } else {
                format!("：{}", sel.ids.join(", "))
            }
        );
    }

    if ctx.dry_run {
        for job in &jobs {
            ctx.events.info(format!(
                "[演练] {} {} → {}\n{}",
                job.kind.label(),
                job.id,
                ctx.project.asset_dir(job.kind, &job.id).display(),
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

    let aspect = ctx.config().aspect;
    let concurrency = ctx.config().generation.image_concurrency;
    let items: Vec<(String, AssetJob)> = jobs.into_iter().map(|j| (j.id.clone(), j)).collect();

    run_items(ctx.events, Stage::Assets, concurrency, items, |id, job| {
        let provider = Arc::clone(&provider);
        async move {
            ctx.check_cancel()?;
            ctx.events
                .item(Stage::Assets, &id, ItemStatus::Generating, "生成中");
            match generate_asset(ctx, provider.as_ref(), &job, aspect, sel.force).await {
                Ok(outcome) => Ok(outcome),
                Err(err) => {
                    let msg = format!("{err:#}");
                    write_meta(ctx.project, &job, ItemStatus::Failed, Some(msg.clone()), &[]);
                    Ok(ItemOutcome::Failed(msg))
                }
            }
        }
    })
    .await
}

fn build_jobs(project: &Project, bd: &Breakdown, sel: &Selection) -> Vec<AssetJob> {
    let style = &project.config.style;
    let mut jobs = Vec::new();

    for ch in &bd.characters {
        if !sel.matches(&ch.id, &ch.name) {
            continue;
        }
        jobs.push(AssetJob {
            kind: AssetKind::Character,
            id: ch.id.clone(),
            name: ch.name.clone(),
            prompt: stored_or(
                project,
                AssetKind::Character,
                &ch.id,
                sel.reset_prompts,
                || prompts::character_prompt(style, ch),
            ),
            views: prompts::CHARACTER_VIEWS
                .iter()
                .map(|(file, desc)| (format!("{file}.png"), desc.to_string()))
                .collect(),
        });
    }

    for c in &bd.costumes {
        if !sel.matches(&c.id, &c.name) {
            continue;
        }
        jobs.push(AssetJob {
            kind: AssetKind::Costume,
            id: c.id.clone(),
            name: c.name.clone(),
            prompt: stored_or(project, AssetKind::Costume, &c.id, sel.reset_prompts, || {
                prompts::costume_prompt(style, c)
            }),
            views: vec![("ref.png".into(), String::new())],
        });
    }

    for p in &bd.props {
        if !sel.matches(&p.id, &p.name) {
            continue;
        }
        jobs.push(AssetJob {
            kind: AssetKind::Prop,
            id: p.id.clone(),
            name: p.name.clone(),
            prompt: stored_or(project, AssetKind::Prop, &p.id, sel.reset_prompts, || {
                prompts::prop_prompt(style, p)
            }),
            views: vec![("ref.png".into(), String::new())],
        });
    }

    for l in &bd.locations {
        if !sel.matches(&l.id, &l.name) {
            continue;
        }
        jobs.push(AssetJob {
            kind: AssetKind::Location,
            id: l.id.clone(),
            name: l.name.clone(),
            prompt: stored_or(project, AssetKind::Location, &l.id, sel.reset_prompts, || {
                prompts::location_prompt(style, l)
            }),
            views: vec![("ref.png".into(), String::new())],
        });
    }

    jobs
}

/// A hand-edited `prompt.txt` wins over the composed prompt — that is the whole
/// point of writing prompts to disk.
fn stored_or(
    project: &Project,
    kind: AssetKind,
    id: &str,
    reset: bool,
    compose: impl FnOnce() -> String,
) -> String {
    if !reset {
        let path = project.asset_dir(kind, id).join("prompt.txt");
        if let Ok(saved) = std::fs::read_to_string(&path) {
            if !saved.trim().is_empty() {
                return saved.trim().to_string();
            }
        }
    }
    compose()
}

async fn generate_asset(
    ctx: &StageCtx<'_>,
    provider: &dyn ImageProvider,
    job: &AssetJob,
    aspect: crate::model::AspectRatio,
    force: bool,
) -> Result<ItemOutcome> {
    let dir = ctx.project.asset_dir(job.kind, &job.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("创建目录 {}", dir.display()))?;

    let mut written = Vec::new();
    let mut generated = 0usize;

    for (file, view_desc) in &job.views {
        ctx.check_cancel()?;
        let out = dir.join(file);
        if out.is_file() && !force {
            written.push(file.clone());
            continue;
        }

        let prompt = if view_desc.is_empty() {
            format!("{}\n无文字、无水印。", job.prompt)
        } else {
            prompts::character_view_prompt(&job.prompt, view_desc)
        };

        let bytes = provider
            .generate(ImageRequest {
                prompt: &prompt,
                aspect,
                references: &[],
            })
            .await
            .with_context(|| format!("{} {} / {file}", job.kind.label(), job.id))?;

        tokio::fs::write(&out, &bytes)
            .await
            .with_context(|| format!("写入 {}", out.display()))?;
        ctx.events.artifact(&out);
        written.push(file.clone());
        generated += 1;
    }

    std::fs::write(dir.join("prompt.txt"), &job.prompt)?;
    write_meta(ctx.project, job, ItemStatus::Done, None, &written);

    Ok(if generated == 0 {
        ItemOutcome::Skipped("已存在，跳过".into())
    } else {
        ItemOutcome::Generated
    })
}

fn write_meta(
    project: &Project,
    job: &AssetJob,
    status: ItemStatus,
    error: Option<String>,
    files: &[String],
) {
    let meta = AssetMeta {
        id: job.id.clone(),
        kind: job.kind.tag().into(),
        name: job.name.clone(),
        prompt: job.prompt.clone(),
        files: files.to_vec(),
        status,
        error,
    };
    let path = project.asset_dir(job.kind, &job.id).join("meta.json");
    if let Ok(text) = serde_json::to_string_pretty(&meta) {
        let _ = crate::model::project::write_atomic(&path, text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AspectRatio, Character, Location};

    fn setup() -> (tempfile::TempDir, Project, Breakdown) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "写实", AspectRatio::Landscape).unwrap();
        let bd = Breakdown {
            characters: vec![Character {
                id: "char_a".into(),
                name: "阿明".into(),
                appearance: "短发".into(),
                costume: String::new(),
                personality: String::new(),
            }],
            locations: vec![Location {
                id: "loc_1".into(),
                name: "仓库".into(),
                description: "废弃仓库".into(),
                time_of_day: "夜".into(),
            }],
            ..Default::default()
        };
        (tmp, proj, bd)
    }

    #[test]
    fn characters_get_three_views_others_get_one() {
        let (_t, proj, bd) = setup();
        let jobs = build_jobs(&proj, &bd, &Selection::default());
        assert_eq!(jobs.len(), 2);
        let ch = jobs.iter().find(|j| j.id == "char_a").unwrap();
        assert_eq!(ch.views.len(), 3);
        let loc = jobs.iter().find(|j| j.id == "loc_1").unwrap();
        assert_eq!(loc.views[0].0, "ref.png");
    }

    #[test]
    fn selection_matches_id_or_name() {
        let (_t, proj, bd) = setup();
        let jobs = build_jobs(&proj, &bd, &Selection::only(vec!["阿明".into()]));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "char_a");
    }

    #[test]
    fn hand_edited_prompt_wins_unless_reset() {
        let (_t, proj, bd) = setup();
        let dir = proj.asset_dir(AssetKind::Character, "char_a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prompt.txt"), "我自己写的 prompt").unwrap();

        let jobs = build_jobs(&proj, &bd, &Selection::default());
        let ch = jobs.iter().find(|j| j.id == "char_a").unwrap();
        assert_eq!(ch.prompt, "我自己写的 prompt");

        let reset = Selection {
            reset_prompts: true,
            ..Default::default()
        };
        let jobs = build_jobs(&proj, &bd, &reset);
        let ch = jobs.iter().find(|j| j.id == "char_a").unwrap();
        assert!(ch.prompt.contains("短发"));
    }
}
