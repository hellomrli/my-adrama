//! The engine: one job dispatcher shared by the CLI and the GUI.
//!
//! Previously each front-end had its own `match` over commands, with its own
//! gating rules that had already drifted apart. There is now exactly one path
//! from "user asked for X" to "files on disk".

pub mod events;
pub mod pipeline;
pub mod prompts;
pub mod stages;
pub mod subtitles;

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::model::{Capability, EndpointMode, Project, ProviderId, Stage, StageStatus};
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
    /// 把剧本整理成标准影视剧本模板（原稿自动备份为 .bak）。
    FormatScript,
    /// 逐镜头配音（云端 TTS 或本地 Piper）。
    Voice(stages::voice::Selection),
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
    /// 这次探测属于哪种能力——密钥与模型列表都按能力隔离存放。
    pub capability: Capability,
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

/// Model ids a connectivity probe discovered, so the UI can offer a picker
/// instead of making the user type an id that may have been renamed upstream.
#[derive(Debug, Clone)]
pub struct ProbedModels {
    pub capability: Capability,
    pub provider: ProviderId,
    pub mode: EndpointMode,
    pub models: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct JobOutcome {
    pub message: String,
    /// Optional multi-line detail (probe results, ffmpeg output path…).
    pub detail: Option<String>,
    /// Stage whose state may have changed, so the UI knows to refresh.
    pub stage: Option<Stage>,
    /// Set by `Job::Probe`.
    pub models: Option<ProbedModels>,
}

impl JobOutcome {
    fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
            stage: None,
            models: None,
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
        Job::FormatScript => "格式化剧本".to_string(),
        Job::Voice(sel) => match sel.shots.len() {
            0 => "生成配音".to_string(),
            1 => format!("重生成配音 {}", sel.shots[0]),
            n => format!("重生成 {n} 条配音"),
        },
        Job::Export => "拼接成片".to_string(),
        Job::Approve(stage) => format!("审核通过 · {}", stage.label()),
        Job::Reset(stage) => format!("撤销审核 · {}", stage.label()),
        Job::Probe(p) => format!(
            "测试 {} · {} {}",
            p.capability.label(),
            p.provider.label(),
            p.mode.label()
        ),
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
            Job::Parse
                | Job::FormatScript
                | Job::Assets(_)
                | Job::Storyboard(_)
                | Job::Video(_)
                | Job::Voice(_)
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
        let mut outcome = JobOutcome::msg(report.summary).detail(report.detail);
        outcome.models = Some(ProbedModels {
            capability: p.capability,
            provider: p.provider,
            mode: p.mode,
            models: report.models,
        });
        return Ok(outcome);
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
        Job::FormatScript => {
            let (path, script) = project.read_script()?;
            let user = prompts::format_user_prompt(&script);
            if dry_run {
                ctx.info("— 演练：以下内容不会发送 —");
                ctx.info(format!("System:\n{}", prompts::FORMAT_SYSTEM));
                ctx.info(format!(
                    "User（{} 字）：\n{}",
                    user.chars().count(),
                    crate::model::index::truncate(&user, 400)
                ));
                return Ok(JobOutcome::msg("格式化演练完成（未调用 API）"));
            }

            let factory = crate::providers::ProviderFactory::new(&project.config, &credentials);
            let provider = factory.chat()?;
            ctx.info(format!("调用 {} 整理剧本格式…", provider.endpoint()));

            let events = ctx.clone();
            let on_progress = move |p: crate::providers::http::SseProgress| {
                events.progress(
                    0,
                    0,
                    format!("接收中 {} 字 · 已用 {} 秒", p.chars, p.elapsed.as_secs()),
                );
            };
            let formatted = provider
                .complete_text(prompts::FORMAT_SYSTEM, &user, Some(&on_progress))
                .await?;
            let formatted = formatted.trim();
            if formatted.chars().count() < script.chars().count() / 3 {
                anyhow::bail!(
                    "格式化结果过短（{} 字，原文 {} 字），怀疑被截断，已放弃写入",
                    formatted.chars().count(),
                    script.chars().count()
                );
            }

            // 原稿备份成 .bak（不在剧本扩展名之列，不会被误当作剧本）
            let backup = path.with_extension(format!(
                "{}.bak",
                path.extension().and_then(|e| e.to_str()).unwrap_or("md")
            ));
            std::fs::copy(&path, &backup)
                .with_context(|| format!("备份原稿到 {}", backup.display()))?;
            crate::model::project::write_atomic(&path, formatted.as_bytes())?;
            ctx.info(format!("原稿已备份 → {}", backup.display()));

            Ok(JobOutcome::msg(format!(
                "剧本已格式化（{} 字），原稿备份为 .bak；请在「剧本」页检查后再拆解",
                formatted.chars().count()
            )))
        }
        Job::Voice(sel) => {
            // 配音不设门控阶段，但至少要有拆解出的台词
            if !dry_run {
                pipeline::require_approved(&project, Stage::Parse)?;
            }
            let stage_ctx = stage_ctx(&project, &credentials, ctx, dry_run);
            let report = stages::voice::run(&stage_ctx, &sel).await?;
            Ok(JobOutcome::msg(stage_message("配音", &report, dry_run)))
        }
        Job::Export => {
            let stage_ctx = StageCtx {
                project: &project,
                credentials: &credentials,
                events: ctx,
                dry_run,
            };
            let report = stages::export::run(&stage_ctx).await?;
            let mut extras = Vec::new();
            if report.mixed_voice {
                extras.push("已混入配音");
            }
            if report.burned_subs {
                extras.push("已烧录字幕");
            }
            let extra = if extras.is_empty() {
                String::new()
            } else {
                format!("，{}", extras.join("、"))
            };
            Ok(
                JobOutcome::msg(format!("成片已生成（{} 个片段{extra}）", report.clips))
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

/// 用户自己准备的素材：图片转成 PNG 放到该条目该在的位置，视频原样复制。
/// 之后这一条会被标记为「手动」，批量重生成不会覆盖它。
pub fn import_item_file(
    root: &std::path::Path,
    stage: Stage,
    id: &str,
    source: &std::path::Path,
) -> Result<PathBuf> {
    use crate::model::project::write_atomic;
    use crate::model::{AssetKind, AssetMeta, ItemStatus, StoryboardMeta, VideoMeta, ASSET_KINDS};

    let project = Project::open(root)?;
    if !source.is_file() {
        anyhow::bail!("找不到文件：{}", source.display());
    }

    match stage {
        Stage::Parse => anyhow::bail!("拆解阶段不支持导入素材"),
        Stage::Assets => {
            let kind = ASSET_KINDS
                .into_iter()
                .find(|k| project.asset_dir(*k, id).is_dir())
                .or_else(|| asset_kind_of(&project, id))
                .with_context(|| format!("找不到资产 {id}"))?;
            let dir = project.asset_dir(kind, id);
            std::fs::create_dir_all(&dir)?;
            // 角色以正面图作为后续分镜的参考，其余用 ref.png。
            let file = if kind == AssetKind::Character {
                "front.png"
            } else {
                "ref.png"
            };
            let target = dir.join(file);
            save_as_png(source, &target)?;

            let meta_path = dir.join("meta.json");
            let mut meta: AssetMeta = std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            meta.id = id.to_string();
            meta.kind = kind.tag().into();
            if meta.name.is_empty() {
                meta.name = id.to_string();
            }
            if !meta.files.iter().any(|f| f == file) {
                meta.files.push(file.to_string());
            }
            meta.status = ItemStatus::Done;
            meta.error = None;
            meta.manual = true;
            write_atomic(&meta_path, serde_json::to_string_pretty(&meta)?.as_bytes())?;
            Ok(target)
        }
        Stage::Storyboard => {
            let target = project.storyboard_image(id);
            save_as_png(source, &target)?;

            let path = project.storyboard_meta(id);
            let mut meta: StoryboardMeta = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            meta.shot_id = id.to_string();
            meta.image = Some(format!("{id}.png"));
            meta.status = ItemStatus::Done;
            meta.error = None;
            meta.manual = true;
            write_atomic(&path, serde_json::to_string_pretty(&meta)?.as_bytes())?;
            Ok(target)
        }
        Stage::Video => {
            let ext = source
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if ext != "mp4" {
                anyhow::bail!("视频请提供 .mp4（当前是 .{ext}）：拼接成片按 mp4 直接复制流处理");
            }
            let target = project.video_clip(id);
            std::fs::create_dir_all(project.video_dir())?;
            std::fs::copy(source, &target)
                .with_context(|| format!("复制到 {}", target.display()))?;

            let path = project.video_meta(id);
            let mut meta: VideoMeta = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            meta.shot_id = id.to_string();
            meta.video = Some(format!("{id}.mp4"));
            meta.status = ItemStatus::Done;
            meta.error = None;
            meta.manual = true;
            meta.operation_name = None;
            write_atomic(&path, serde_json::to_string_pretty(&meta)?.as_bytes())?;
            Ok(target)
        }
    }
}

/// 统一转成 PNG：流水线各处都按 .png 找文件，jpg/webp 直接改名会找不到。
fn save_as_png(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(source)
        .with_context(|| format!("读取 {}", source.display()))?;
    let image = image::load_from_memory(&bytes)
        .with_context(|| format!("无法识别的图片格式：{}", source.display()))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    image
        .save_with_format(target, image::ImageFormat::Png)
        .with_context(|| format!("写入 {}", target.display()))
}

/// 把整个项目目录打成 tar.gz（Linux 的 tar 与 Windows 10+ 自带的 bsdtar 都认）。
/// 密钥不在项目目录里，按设计不随包走。
pub fn pack_project(root: &std::path::Path, dest_dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let project = Project::open(root)?;
    let name = project
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();
    let parent = project
        .root
        .parent()
        .with_context(|| "项目目录没有上级目录")?;
    std::fs::create_dir_all(dest_dir)?;
    let archive = dest_dir.join(format!("{name}.adrama.tar.gz"));

    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(parent)
        .arg(&name)
        .status()
        .context("未找到 tar（Linux 自带；Windows 10+ 也自带）")?;
    if !status.success() {
        anyhow::bail!("打包失败（tar 退出码 {status}）");
    }
    Ok(archive)
}

/// 解开项目包并返回项目目录（找包里含 project.toml 的目录）。
pub fn unpack_project(
    archive: &std::path::Path,
    parent: &std::path::Path,
) -> Result<std::path::PathBuf> {
    if !archive.is_file() {
        anyhow::bail!("找不到项目包：{}", archive.display());
    }
    std::fs::create_dir_all(parent)?;
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(parent)
        .status()
        .context("未找到 tar（Linux 自带；Windows 10+ 也自带）")?;
    if !status.success() {
        anyhow::bail!("解包失败（tar 退出码 {status}）");
    }
    // 在 parent 下找刚解出来的项目目录
    for entry in std::fs::read_dir(parent)?.flatten() {
        let path = entry.path();
        if path.is_dir() && Project::is_project(&path) {
            if let Ok(project) = Project::open(&path) {
                return Ok(project.root);
            }
        }
    }
    anyhow::bail!("包里没有找到 adrama 项目（缺 project.toml）")
}

/// 生成 SRT 字幕文件，返回路径与条数。
pub fn write_subtitles(root: &std::path::Path) -> Result<(std::path::PathBuf, usize)> {
    let project = Project::open(root)?;
    let bd = project.load_breakdown()?;
    let (text, count) = subtitles::srt(&bd);
    if count == 0 {
        anyhow::bail!("没有台词，无法生成字幕（拆解里 dialogue 都为空）");
    }
    let path = project.subtitles_path();
    crate::model::project::write_atomic(&path, text.as_bytes())?;
    Ok((path, count))
}

/// 导入用户自己的配音（mp3/wav），标记为手动，批量生成不覆盖。
pub fn import_voice_file(
    root: &std::path::Path,
    shot_id: &str,
    source: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use crate::model::{ItemStatus, VoiceMeta};

    let project = Project::open(root)?;
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "mp3" | "wav") {
        anyhow::bail!("配音请提供 .mp3 或 .wav（当前是 .{ext}）");
    }
    std::fs::create_dir_all(project.voice_dir())?;
    // 清掉另一种扩展名的旧文件，避免两个都在时取错
    for other in ["mp3", "wav"] {
        if other != ext {
            let _ = std::fs::remove_file(project.voice_dir().join(format!("{shot_id}.{other}")));
        }
    }
    let target = project.voice_dir().join(format!("{shot_id}.{ext}"));
    std::fs::copy(source, &target)
        .with_context(|| format!("复制到 {}", target.display()))?;

    let meta = VoiceMeta {
        shot_id: shot_id.to_string(),
        text: String::new(),
        voice: "手动导入".into(),
        model: String::new(),
        status: ItemStatus::Done,
        error: None,
        manual: true,
    };
    crate::model::project::write_atomic(
        &project.voice_meta(shot_id),
        serde_json::to_string_pretty(&meta)?.as_bytes(),
    )?;
    Ok(target)
}

/// 设置某个镜头的分镜帧数覆盖（None = 恢复跟随全局）。
pub fn set_shot_frames(root: &std::path::Path, shot_id: &str, frames: Option<u32>) -> Result<()> {
    use crate::model::StoryboardMeta;

    let project = Project::open(root)?;
    let path = project.storyboard_meta(shot_id);
    let mut meta: StoryboardMeta = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    meta.shot_id = shot_id.to_string();
    meta.frames = frames.map(|n| n.clamp(2, 8));
    crate::model::project::write_atomic(&path, serde_json::to_string_pretty(&meta)?.as_bytes())
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
                "「{}」选的是 {}，但它不提供该能力（设置 → 模型与密钥）",
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
                visual_end: String::new(),
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
