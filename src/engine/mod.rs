//! The engine: one job dispatcher shared by the CLI and the GUI.
//!
//! Previously each front-end had its own `match` over commands, with its own
//! gating rules that had already drifted apart. There is now exactly one path
//! from "user asked for X" to "files on disk".

pub mod events;
pub mod pipeline;
pub mod prompts;
pub mod stages;

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::model::{EndpointMode, Project, ProviderId, Stage, StageStatus};
use crate::providers::{self, Credentials};
use events::{JobContext, StageEvent};
use stages::StageCtx;

/// A unit of work a front-end can ask for.
#[derive(Debug, Clone)]
pub enum Job {
    Parse,
    Assets(stages::assets::Selection),
    Storyboard(stages::storyboard::Selection),
    Video(stages::video::Selection),
    /// ffmpeg concatenation of existing clips.
    Export,
    Approve(Stage),
    /// Revoke an approval (and any downstream ones).
    Reset(Stage),
    /// Settings-screen connectivity check; touches no project files.
    Probe(ProbeRequest),
}

#[derive(Debug, Clone)]
pub struct ProbeRequest {
    pub provider: ProviderId,
    pub mode: EndpointMode,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct JobRequest {
    pub root: PathBuf,
    pub job: Job,
    /// Assemble prompts, print them, call nothing.
    pub dry_run: bool,
    pub credentials: Credentials,
}

#[derive(Debug, Clone)]
pub struct JobOutcome {
    pub message: String,
    /// Optional multi-line detail (probe results, ffmpeg output path…).
    pub detail: Option<String>,
    /// Stage whose state may have changed, so the UI knows to refresh.
    pub stage: Option<Stage>,
}

impl JobOutcome {
    fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
            stage: None,
        }
    }

    fn stage(mut self, stage: Stage) -> Self {
        self.stage = Some(stage);
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// User-facing name for a job, used in logs, the CLI and the busy indicator.
pub fn job_label(job: &Job, dry_run: bool) -> String {
    let base = match job {
        Job::Parse => "解析剧本".to_string(),
        Job::Assets(sel) => match sel.ids.len() {
            0 => "生成资产".to_string(),
            1 => format!("重生成资产 {}", sel.ids[0]),
            n => format!("重生成 {n} 项资产"),
        },
        Job::Storyboard(sel) => match (sel.shots.len(), sel.scene) {
            (0, None) => "生成分镜".to_string(),
            (0, Some(n)) => format!("生成第 {n} 场分镜"),
            (1, _) => format!("重生成分镜 {}", sel.shots[0]),
            (n, _) => format!("重生成 {n} 个分镜"),
        },
        Job::Video(sel) => match sel.shots.len() {
            0 => "生成视频".to_string(),
            1 => format!("重生成视频 {}", sel.shots[0]),
            n => format!("重生成 {n} 个视频"),
        },
        Job::Export => "拼接成片".to_string(),
        Job::Approve(stage) => format!("审核通过 · {}", stage.label()),
        Job::Reset(stage) => format!("撤销审核 · {}", stage.label()),
        Job::Probe(p) => format!("测试连接 · {} {}", p.provider.label(), p.mode.label()),
    };
    if dry_run && job.touches_api() {
        format!("{base}（演练）")
    } else {
        base
    }
}

impl Job {
    /// Does this job spend money when not in dry-run mode?
    pub fn touches_api(&self) -> bool {
        matches!(
            self,
            Job::Parse | Job::Assets(_) | Job::Storyboard(_) | Job::Video(_)
        )
    }

}

/// Run a job start to finish, emitting `Started` / `Finished` around it.
pub async fn run_job(req: JobRequest, ctx: &JobContext) -> Result<JobOutcome> {
    let label = job_label(&req.job, req.dry_run);
    ctx.sink.emit(StageEvent::Started {
        label: label.clone(),
    });

    let result = execute(req, ctx).await;
    match &result {
        Ok(outcome) => ctx.sink.emit(StageEvent::Finished {
            ok: true,
            message: outcome.message.clone(),
        }),
        Err(err) => ctx.sink.emit(StageEvent::Finished {
            ok: false,
            message: format!("{label} 失败：{err:#}"),
        }),
    }
    result
}

/// Dispatch without the surrounding events (used by tests).
pub async fn execute(req: JobRequest, ctx: &JobContext) -> Result<JobOutcome> {
    let JobRequest {
        root,
        job,
        dry_run,
        credentials,
    } = req;

    // Connectivity checks need no project.
    if let Job::Probe(p) = &job {
        let report = providers::probe(p.provider, p.mode, &p.base_url, &p.api_key, &p.model).await?;
        ctx.info(report.detail.clone());
        return Ok(JobOutcome::msg(report.summary).detail(report.detail));
    }

    let mut project = Project::open(&root)?;
    ctx.check_routing(&project);

    match job {
        Job::Approve(stage) => {
            pipeline::approve(&mut project, stage)?;
            let next = stage
                .next()
                .map(|n| format!("，下一步：{}", n.label()))
                .unwrap_or_default();
            Ok(JobOutcome::msg(format!("已审核通过：{}{next}", stage.label())).stage(stage))
        }
        Job::Reset(stage) => {
            pipeline::reset(&mut project, stage)?;
            Ok(JobOutcome::msg(format!("已撤销审核：{}", stage.label())).stage(stage))
        }
        Job::Export => {
            let stage_ctx = StageCtx {
                project: &project,
                credentials: &credentials,
                events: ctx,
                dry_run,
            };
            let report = stages::export::run(&stage_ctx).await?;
            Ok(
                JobOutcome::msg(format!("成片已生成（{} 个片段）", report.clips))
                    .detail(report.output.display().to_string())
                    .stage(Stage::Video),
            )
        }
        Job::Parse => {
            begin(&mut project, Stage::Parse, dry_run)?;
            let result = {
                let stage_ctx = stage_ctx(&project, &credentials, ctx, dry_run);
                stages::parse::run(&stage_ctx).await
            };
            finish(&mut project, Stage::Parse, dry_run);
            let bd = result?;
            Ok(if dry_run {
                JobOutcome::msg("解析演练完成（未调用 API）")
            } else {
                JobOutcome::msg(format!(
                    "解析完成：{} 角色 · {} 镜头，请检查后点击「审核通过」",
                    bd.characters.len(),
                    bd.shots.len()
                ))
            }
            .stage(Stage::Parse))
        }
        Job::Assets(sel) => {
            begin(&mut project, Stage::Assets, dry_run)?;
            let result = {
                let stage_ctx = stage_ctx(&project, &credentials, ctx, dry_run);
                stages::assets::run(&stage_ctx, &sel).await
            };
            finish(&mut project, Stage::Assets, dry_run);
            let report = result?;
            Ok(JobOutcome::msg(stage_message("资产", &report, dry_run)).stage(Stage::Assets))
        }
        Job::Storyboard(sel) => {
            begin(&mut project, Stage::Storyboard, dry_run)?;
            let result = {
                let stage_ctx = stage_ctx(&project, &credentials, ctx, dry_run);
                stages::storyboard::run(&stage_ctx, &sel).await
            };
            finish(&mut project, Stage::Storyboard, dry_run);
            let report = result?;
            Ok(JobOutcome::msg(stage_message("分镜", &report, dry_run)).stage(Stage::Storyboard))
        }
        Job::Video(sel) => {
            begin(&mut project, Stage::Video, dry_run)?;
            let result = {
                let stage_ctx = stage_ctx(&project, &credentials, ctx, dry_run);
                stages::video::run(&stage_ctx, &sel).await
            };
            finish(&mut project, Stage::Video, dry_run);
            let report = result?;
            Ok(JobOutcome::msg(stage_message("视频", &report, dry_run)).stage(Stage::Video))
        }
        Job::Probe(_) => unreachable!("handled above"),
    }
}

/// Persist a hand-edited prompt for a single item so the next run uses it.
/// An empty `text` clears the override and restores the composed default.
pub fn save_prompt(root: &std::path::Path, stage: Stage, id: &str, text: &str) -> Result<()> {
    use crate::model::project::write_atomic;
    use crate::model::{AssetMeta, StoryboardMeta, VideoMeta, ASSET_KINDS};

    let project = Project::open(root)?;
    let text = text.trim();

    match stage {
        Stage::Parse => anyhow::bail!("拆解阶段没有单条 prompt"),
        Stage::Assets => {
            // The directory only exists once the asset has been generated, so
            // fall back to the breakdown to learn which family this id is in.
            let kind = ASSET_KINDS
                .into_iter()
                .find(|k| project.asset_dir(*k, id).is_dir())
                .or_else(|| asset_kind_of(&project, id))
                .with_context(|| format!("找不到资产 {id}"))?;
            let dir = project.asset_dir(kind, id);
            std::fs::create_dir_all(&dir)?;
            let prompt_path = dir.join("prompt.txt");
            if text.is_empty() {
                let _ = std::fs::remove_file(&prompt_path);
            } else {
                write_atomic(&prompt_path, text.as_bytes())?;
            }

            let meta_path = dir.join("meta.json");
            if let Ok(raw) = std::fs::read_to_string(&meta_path) {
                if let Ok(mut meta) = serde_json::from_str::<AssetMeta>(&raw) {
                    meta.prompt = text.to_string();
                    write_atomic(&meta_path, serde_json::to_string_pretty(&meta)?.as_bytes())?;
                }
            }
            Ok(())
        }
        Stage::Storyboard => {
            let path = project.storyboard_meta(id);
            let mut meta: StoryboardMeta = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            meta.shot_id = id.to_string();
            meta.prompt = text.to_string();
            write_atomic(&path, serde_json::to_string_pretty(&meta)?.as_bytes())
        }
        Stage::Video => {
            let path = project.video_meta(id);
            let mut meta: VideoMeta = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            meta.shot_id = id.to_string();
            meta.prompt = text.to_string();
            write_atomic(&path, serde_json::to_string_pretty(&meta)?.as_bytes())
        }
    }
}

/// Which asset family an id belongs to, according to the breakdown.
fn asset_kind_of(project: &Project, id: &str) -> Option<crate::model::AssetKind> {
    use crate::model::AssetKind;
    let bd = project.load_breakdown().ok()?;
    if bd.characters.iter().any(|c| c.id == id) {
        return Some(AssetKind::Character);
    }
    if bd.costumes.iter().any(|c| c.id == id) {
        return Some(AssetKind::Costume);
    }
    if bd.props.iter().any(|p| p.id == id) {
        return Some(AssetKind::Prop);
    }
    if bd.locations.iter().any(|l| l.id == id) {
        return Some(AssetKind::Location);
    }
    None
}

/// Fill in the prompt each not-yet-generated item *would* use, so the UI can
/// show and edit it before spending anything.
pub fn fill_default_prompts(
    config: &crate::model::ProjectConfig,
    bd: &crate::model::Breakdown,
    index: &mut crate::model::ProjectIndex,
) {
    use crate::model::index::ItemKind;
    use crate::model::AssetKind;

    let style = &config.style;
    for item in &mut index.assets {
        if !item.prompt.trim().is_empty() {
            continue;
        }
        item.prompt = match item.kind {
            ItemKind::Asset(AssetKind::Character) => bd
                .characters
                .iter()
                .find(|c| c.id == item.id)
                .map(|c| prompts::character_prompt(style, c)),
            ItemKind::Asset(AssetKind::Costume) => bd
                .costumes
                .iter()
                .find(|c| c.id == item.id)
                .map(|c| prompts::costume_prompt(style, c)),
            ItemKind::Asset(AssetKind::Prop) => bd
                .props
                .iter()
                .find(|p| p.id == item.id)
                .map(|p| prompts::prop_prompt(style, p)),
            ItemKind::Asset(AssetKind::Location) => bd
                .locations
                .iter()
                .find(|l| l.id == item.id)
                .map(|l| prompts::location_prompt(style, l)),
            _ => None,
        }
        .unwrap_or_default();
    }

    for item in &mut index.storyboard {
        if item.prompt.trim().is_empty() {
            if let Some(shot) = bd.shots.iter().find(|s| s.id == item.id) {
                item.prompt = prompts::storyboard_prompt(style, bd, shot);
            }
        }
    }
    for item in &mut index.videos {
        if item.prompt.trim().is_empty() {
            if let Some(shot) = bd.shots.iter().find(|s| s.id == item.id) {
                item.prompt = prompts::video_prompt(shot);
            }
        }
    }
}

/// Dry runs report how much *would* run; real runs report what happened.
fn stage_message(stage: &str, report: &stages::StageReport, dry_run: bool) -> String {
    if dry_run {
        format!("{stage}演练完成：{} 项（未调用 API）", report.skipped)
    } else {
        format!("{stage}：{}", report.summary())
    }
}

fn stage_ctx<'a>(
    project: &'a Project,
    credentials: &'a Credentials,
    events: &'a JobContext,
    dry_run: bool,
) -> StageCtx<'a> {
    StageCtx {
        project,
        credentials,
        events,
        dry_run,
    }
}

/// Gate on the previous stage and flag the stage as running.
fn begin(project: &mut Project, stage: Stage, dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    pipeline::require_ready(project, stage)?;
    pipeline::mark(project, stage, StageStatus::InProgress)
}

/// Settle the stage status from what actually landed on disk.
fn finish(project: &mut Project, stage: Stage, dry_run: bool) {
    if !dry_run {
        settle(project, stage);
    }
}

/// After a run, the stage status must reflect reality, not intent.
fn settle(project: &mut Project, stage: Stage) {
    if project.state.get(stage).is_approved() {
        return;
    }
    let status = if pipeline::output_summary(project, stage).has_output {
        StageStatus::Done
    } else {
        StageStatus::Pending
    };
    let _ = pipeline::mark(project, stage, status);
}

impl JobContext {
    /// Warn once per job when routing points a capability at a provider that
    /// cannot serve it — the user is one click away from a confusing failure.
    fn check_routing(&self, project: &Project) {
        for (cap, provider) in project.config.routing_conflicts() {
            self.warn(format!(
                "「{}」能力被路由到 {}，但它不提供该能力（设置 → 能力路由）",
                cap.label(),
                provider.label()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AspectRatio, Breakdown, Shot};

    fn project(tmp: &tempfile::TempDir) -> Project {
        Project::create(&tmp.path().join("p"), "p", "写实", AspectRatio::Landscape).unwrap()
    }

    fn breakdown() -> Breakdown {
        Breakdown {
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
        }
    }

    #[tokio::test]
    async fn gating_blocks_stage_two_until_stage_one_is_approved() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project(&tmp);
        proj.save_breakdown(&breakdown()).unwrap();

        let ctx = JobContext::null();
        let err = execute(
            JobRequest {
                root: proj.root.clone(),
                job: Job::Assets(stages::assets::Selection::default()),
                dry_run: false,
                credentials: Credentials::default(),
            },
            &ctx,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("解析"), "{err}");
    }

    #[tokio::test]
    async fn dry_run_needs_no_credentials_and_changes_no_state() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project(&tmp);
        proj.save_breakdown(&breakdown()).unwrap();
        proj.write_script("场景一").unwrap();

        let ctx = JobContext::null();
        let outcome = execute(
            JobRequest {
                root: proj.root.clone(),
                job: Job::Parse,
                dry_run: true,
                credentials: Credentials::default(),
            },
            &ctx,
        )
        .await
        .unwrap();

        assert!(outcome.message.contains("演练"));
        let reopened = Project::open(&proj.root).unwrap();
        assert_eq!(reopened.state.parse, StageStatus::Pending);
    }

    #[tokio::test]
    async fn approve_then_reset_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project(&tmp);
        proj.save_breakdown(&breakdown()).unwrap();
        let ctx = JobContext::null();

        let outcome = execute(
            JobRequest {
                root: proj.root.clone(),
                job: Job::Approve(Stage::Parse),
                dry_run: false,
                credentials: Credentials::default(),
            },
            &ctx,
        )
        .await
        .unwrap();
        assert!(outcome.message.contains("已审核通过"));
        assert_eq!(
            Project::open(&proj.root).unwrap().state.parse,
            StageStatus::Approved
        );

        execute(
            JobRequest {
                root: proj.root.clone(),
                job: Job::Reset(Stage::Parse),
                dry_run: false,
                credentials: Credentials::default(),
            },
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(
            Project::open(&proj.root).unwrap().state.parse,
            StageStatus::Done
        );
    }

    #[test]
    fn prompt_can_be_edited_before_the_item_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project(&tmp);
        let mut bd = breakdown();
        bd.characters.push(crate::model::Character {
            id: "char_a".into(),
            name: "阿明".into(),
            appearance: "短发".into(),
            costume: String::new(),
            personality: String::new(),
        });
        proj.save_breakdown(&bd).unwrap();

        // Nothing generated yet: the asset directory does not exist.
        assert!(!proj
            .asset_dir(crate::model::AssetKind::Character, "char_a")
            .exists());

        save_prompt(&proj.root, Stage::Assets, "char_a", "我改过的提示词").unwrap();
        let saved = std::fs::read_to_string(
            proj.asset_dir(crate::model::AssetKind::Character, "char_a")
                .join("prompt.txt"),
        )
        .unwrap();
        assert_eq!(saved, "我改过的提示词");

        // Clearing restores the composed default.
        save_prompt(&proj.root, Stage::Assets, "char_a", "  ").unwrap();
        assert!(!proj
            .asset_dir(crate::model::AssetKind::Character, "char_a")
            .join("prompt.txt")
            .exists());
    }

    #[test]
    fn storyboard_prompt_edits_land_in_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project(&tmp);
        proj.save_breakdown(&breakdown()).unwrap();

        save_prompt(&proj.root, Stage::Storyboard, "shot_1", "定制分镜提示词").unwrap();
        let meta: crate::model::StoryboardMeta = serde_json::from_str(
            &std::fs::read_to_string(proj.storyboard_meta("shot_1")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta.shot_id, "shot_1");
        assert_eq!(meta.prompt, "定制分镜提示词");
    }

    #[test]
    fn pending_items_show_the_prompt_they_would_use() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project(&tmp);
        let mut bd = breakdown();
        bd.characters.push(crate::model::Character {
            id: "char_a".into(),
            name: "阿明".into(),
            appearance: "左眉有疤".into(),
            costume: String::new(),
            personality: String::new(),
        });
        proj.save_breakdown(&bd).unwrap();

        let mut index = crate::model::ProjectIndex::build(&proj, Some(&bd));
        assert!(index.assets[0].prompt.is_empty());
        fill_default_prompts(&proj.config, &bd, &mut index);

        assert!(index.assets[0].prompt.contains("左眉有疤"));
        assert!(index.storyboard[0].prompt.contains("画面"));
        assert!(index.videos[0].prompt.contains("首帧"));
    }

    #[test]
    fn labels_mark_dry_runs_only_for_api_jobs() {
        assert_eq!(job_label(&Job::Parse, true), "解析剧本（演练）");
        assert_eq!(
            job_label(&Job::Approve(Stage::Parse), true),
            "审核通过 · 解析"
        );
        assert_eq!(
            job_label(&Job::Video(stages::video::Selection::only(vec!["s1".into()])), false),
            "重生成视频 s1"
        );
    }
}
