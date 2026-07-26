//! Command-line front-end. Thin: parse arguments, build a [`Job`], render the
//! event stream. All behaviour lives in `engine`.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::engine::events::{CancelToken, EventSink, JobContext, Level, StageEvent};
use crate::engine::{self, Job, JobRequest, ProbeRequest};
use crate::model::{
    AspectRatio, Capability, EndpointMode, Project, ProjectIndex, ProviderId, Stage,
};
use crate::settings::AppSettings;

#[derive(Parser, Debug)]
#[command(
    name = "adrama",
    version,
    about = "AI 短剧生产工作流（GUI + CLI）",
    long_about = "剧本 → 解析 → 资产 → 分镜 → 视频。无子命令时启动图形界面。"
)]
pub struct Cli {
    /// 项目目录（默认当前目录）
    #[arg(long, global = true, default_value = ".")]
    pub project: PathBuf,

    /// 强制打开图形界面
    #[arg(long, global = true)]
    pub gui: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 新建项目目录
    New {
        /// 项目名称 / 目录
        name: String,
        /// 图像提示词的风格前缀
        #[arg(long, default_value = "cinematic, photorealistic, film grain")]
        style: String,
        /// 画幅：16:9 / 9:16 / 1:1
        #[arg(long, default_value = "16:9")]
        aspect: String,
    },
    /// 导入剧本文件
    Import { script: PathBuf },
    /// 阶段 1：解析剧本为结构化 JSON
    Parse {
        #[arg(long)]
        dry_run: bool,
    },
    /// 阶段 2：生成角色 / 服装 / 道具 / 场景资产图
    Assets {
        /// 只处理这些 id 或名称（可重复）
        #[arg(long = "only", value_name = "ID")]
        ids: Vec<String>,
        /// 覆盖已存在的图片
        #[arg(long)]
        force: bool,
        /// 丢弃已保存的 prompt，按 breakdown 重新组装
        #[arg(long)]
        reset_prompt: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// 阶段 3：生成分镜图
    Storyboard {
        /// 只生成某一场（1 起）
        #[arg(long)]
        scene: Option<u32>,
        /// 只生成这些镜头 id（可重复）
        #[arg(long = "shot", value_name = "ID")]
        shots: Vec<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        reset_prompt: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// 阶段 4：分镜图生成视频片段
    Video {
        #[arg(long = "shot", value_name = "ID")]
        shots: Vec<String>,
        #[arg(long)]
        force: bool,
        /// 生成后立即拼接成片
        #[arg(long)]
        concat: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// 用 ffmpeg 把已有片段拼成 final.mp4
    Export {
        #[arg(long)]
        dry_run: bool,
    },
    /// 查看项目状态
    Status,
    /// 查看能力路由与密钥配置
    Providers,
    /// 审核通过某阶段，解锁下一阶段
    Approve { stage: String },
    /// 撤销某阶段的审核
    Reset { stage: String },
    /// 测试某个服务商的连通性
    Test {
        /// openai / google / xai
        provider: String,
        /// official / custom
        #[arg(long, default_value = "official")]
        mode: String,
    },
    /// 检查（或安装）新版本
    Update {
        /// 下载并安装，而不只是检查
        #[arg(long)]
        apply: bool,
    },
    /// 列出某个服务商当前提供的模型
    Models {
        /// openai / google / xai
        provider: String,
        /// official / custom
        #[arg(long, default_value = "official")]
        mode: String,
        /// 只看某种能力的候选：chat / image / video
        #[arg(long)]
        capability: Option<String>,
    },
    /// 打开图形界面
    Gui,
}

/// A failure the event sink has already printed; `main` exits quietly.
#[derive(Debug)]
pub struct AlreadyReported;

impl std::fmt::Display for AlreadyReported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("命令失败")
    }
}

impl std::error::Error for AlreadyReported {}

pub async fn run(cli: Cli) -> Result<()> {
    let settings = AppSettings::load();

    match cli.command.expect("CLI 路径必然有子命令") {
        Command::New {
            name,
            style,
            aspect,
        } => {
            let path = PathBuf::from(&name);
            let display = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&name)
                .to_string();
            let project =
                Project::create(&path, &display, &style, AspectRatio::parse_lossy(&aspect))?;
            println!("已创建项目：{}", project.root.display());
            println!("下一步：adrama import <剧本文件> && adrama parse");
            Ok(())
        }
        Command::Import { script } => {
            let project = Project::open(&cli.project)?;
            let dest = project.import_script(&script)?;
            println!("已导入剧本 → {}", dest.display());
            Ok(())
        }
        Command::Status => {
            let project = Project::open(&cli.project)?;
            print_status(&project);
            Ok(())
        }
        Command::Providers => {
            let project = Project::open(&cli.project)?;
            print_providers(&project, &settings);
            Ok(())
        }
        Command::Test { provider, mode } => {
            let project = Project::open(&cli.project).ok();
            let id: ProviderId = provider.parse()?;
            let mode = parse_mode(&mode)?;
            let config = project
                .as_ref()
                .map(|p| p.config.clone())
                .unwrap_or_default();
            let settings_for_provider = config.provider(id);
            let base_url = match mode {
                EndpointMode::Official => id.official_base_url().to_string(),
                EndpointMode::Custom => settings_for_provider.custom_base_url.clone(),
            };
            let credentials = settings.credentials();
            let api_key = credentials
                .get(id, mode)
                .map(str::to_string)
                .with_context(|| format!("未配置 {} 的{}密钥", id.label(), mode.label()))?;

            dispatch(
                &cli.project,
                Job::Probe(ProbeRequest {
                    provider: id,
                    mode,
                    base_url,
                    api_key,
                    model: settings_for_provider.chat_model.clone(),
                }),
                false,
                &settings,
            )
            .await
        }
        Command::Approve { stage } => {
            dispatch(&cli.project, Job::Approve(parse_stage(&stage)?), false, &settings).await
        }
        Command::Reset { stage } => {
            dispatch(&cli.project, Job::Reset(parse_stage(&stage)?), false, &settings).await
        }
        Command::Parse { dry_run } => dispatch(&cli.project, Job::Parse, dry_run, &settings).await,
        Command::Assets {
            ids,
            force,
            reset_prompt,
            dry_run,
        } => {
            let job = Job::Assets(engine::stages::assets::Selection {
                force: force || !ids.is_empty(),
                ids,
                reset_prompts: reset_prompt,
            });
            dispatch(&cli.project, job, dry_run, &settings).await
        }
        Command::Storyboard {
            scene,
            shots,
            force,
            reset_prompt,
            dry_run,
        } => {
            let job = Job::Storyboard(engine::stages::storyboard::Selection {
                force: force || !shots.is_empty(),
                shots,
                scene,
                reset_prompts: reset_prompt,
            });
            dispatch(&cli.project, job, dry_run, &settings).await
        }
        Command::Video {
            shots,
            force,
            concat,
            dry_run,
        } => {
            let job = Job::Video(engine::stages::video::Selection {
                force: force || !shots.is_empty(),
                shots,
            });
            dispatch(&cli.project, job, dry_run, &settings).await?;
            if concat {
                dispatch(&cli.project, Job::Export, dry_run, &settings).await?;
            }
            Ok(())
        }
        Command::Export { dry_run } => dispatch(&cli.project, Job::Export, dry_run, &settings).await,
        Command::Update { apply } => update_command(apply).await,
        Command::Models {
            provider,
            mode,
            capability,
        } => models_command(&cli.project, &provider, &mode, capability.as_deref(), &settings).await,
        Command::Gui => unreachable!("GUI 在 main 中提前处理"),
    }
}

async fn dispatch(root: &Path, job: Job, dry_run: bool, settings: &AppSettings) -> Result<()> {
    let sink = Arc::new(CliSink::new());
    let ctx = JobContext::new(sink.clone(), CancelToken::new());
    // The sink prints the failure as it happens; do not print it twice.
    let outcome = engine::run_job(
        JobRequest {
            root: root.to_path_buf(),
            job,
            dry_run,
            credentials: settings.credentials(),
        },
        &ctx,
    )
    .await
    .map_err(|_| AlreadyReported)?;

    if let Some(detail) = outcome.detail {
        println!("{detail}");
    }
    Ok(())
}

async fn update_command(apply: bool) -> Result<()> {
    let install = crate::update::install_kind();
    println!("当前版本  {}", crate::update::CURRENT_VERSION);
    println!("安装方式  {}", install.describe());

    let release = match crate::update::check().await? {
        crate::update::UpdateStatus::UpToDate => {
            println!("已是最新版本");
            return Ok(());
        }
        crate::update::UpdateStatus::Available(release) => release,
    };

    println!("新版本    {} （{}）", release.version, release.page_url);
    for line in release.notes.lines().take(12) {
        println!("  {line}");
    }
    if !apply {
        println!();
        println!("加 --apply 下载并安装；或直接从上面的发布页下载。");
        return Ok(());
    }

    if !install.can_self_update() {
        anyhow::bail!("{}", install.describe());
    }

    let bar = ProgressBar::new(0);
    bar.set_style(
        ProgressStyle::with_template("{spinner} [{bar:24}] {bytes}/{total_bytes} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    bar.set_message("下载中");

    let applied = crate::update::download_and_apply(&release, |received, total| {
        bar.set_length(total);
        bar.set_position(received);
    })
    .await?;
    bar.finish_and_clear();

    println!(
        "✔ 已更新到 {}{}",
        applied.version,
        if applied.verified {
            "（SHA-256 校验通过）"
        } else {
            "（该 release 未提供校验和）"
        }
    );
    println!("重新运行 {} 即可使用新版本。", applied.executable.display());
    Ok(())
}

async fn models_command(
    project_dir: &Path,
    provider: &str,
    mode: &str,
    capability: Option<&str>,
    settings: &AppSettings,
) -> Result<()> {
    let id: ProviderId = provider.parse()?;
    let mode = parse_mode(mode)?;
    let config = Project::open(project_dir)
        .map(|p| p.config)
        .unwrap_or_default();
    let provider_settings = config.provider(id);
    let base_url = match mode {
        EndpointMode::Official => id.official_base_url().to_string(),
        EndpointMode::Custom => provider_settings.custom_base_url.clone(),
    };
    let api_key = settings
        .credentials()
        .get(id, mode)
        .map(str::to_string)
        .with_context(|| format!("未配置 {} 的{}密钥", id.label(), mode.label()))?;

    let report = crate::providers::probe(id, mode, &base_url, &api_key, "").await?;
    if report.models.is_empty() {
        println!("该端点没有返回模型列表（代理可能未实现 /models）");
        return Ok(());
    }

    let filter = capability.map(|c| c.parse::<Capability>()).transpose()?;
    println!("{} · {} — {} 个模型", id.label(), mode.label(), report.models.len());
    for model in &report.models {
        match filter {
            Some(cap) if !crate::providers::looks_like(cap, model) => continue,
            _ => println!("  {model}"),
        }
    }
    if filter.is_some() {
        println!();
        println!("（按名称启发式筛选，未列出的也可以手动填写）");
    }
    Ok(())
}

fn parse_stage(s: &str) -> Result<Stage> {
    s.parse()
}

fn parse_mode(s: &str) -> Result<EndpointMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "official" | "官方" => Ok(EndpointMode::Official),
        "custom" | "自定义" => Ok(EndpointMode::Custom),
        other => anyhow::bail!("未知端点模式：{other}（official / custom）"),
    }
}

fn print_status(project: &Project) {
    let breakdown = project.load_breakdown().ok();
    let index = ProjectIndex::build(project, breakdown.as_ref());

    println!("项目  {}", project.config.name);
    println!("路径  {}", project.root.display());
    println!("风格  {}", project.config.style);
    println!("画幅  {}", project.config.aspect);
    println!();

    println!("阶段");
    for stage in Stage::ALL {
        let status = project.state.get(stage);
        let extra = match stage {
            Stage::Parse => breakdown
                .as_ref()
                .map(|b| format!("{} 镜头", b.shots.len()))
                .unwrap_or_else(|| "—".into()),
            other => index.counts(other).summary(),
        };
        println!(
            "  {} {}  {:<6} {}",
            status.glyph(),
            stage.ordinal(),
            stage.label(),
            format_args!("{:<6} {extra}", status.label())
        );
    }

    if let Some(bd) = &breakdown {
        println!();
        println!(
            "结构  {} 角色 · {} 场景 · {} 场 · {} 镜头 · 约 {} 秒",
            bd.characters.len(),
            bd.locations.len(),
            bd.scenes.len(),
            bd.shots.len(),
            bd.total_seconds()
        );
        let issues = bd.lint();
        if !issues.is_empty() {
            println!();
            println!("提醒");
            for issue in issues.iter().take(8) {
                println!("  ! {issue}");
            }
        }
    }

    if let Some(final_cut) = &index.final_cut {
        println!();
        println!("成片  {}", final_cut.display());
    }
}

fn print_providers(project: &Project, settings: &AppSettings) {
    let credentials = settings.credentials();
    println!("能力路由");
    for cap in Capability::ALL {
        let endpoint = project.config.endpoint(cap);
        let ok = if endpoint.provider.supports(cap) {
            if credentials.has(endpoint.provider, endpoint.mode) {
                "✓"
            } else {
                "! 缺少密钥"
            }
        } else {
            "✗ 该服务商不支持此能力"
        };
        println!(
            "  {:<4} {:<8} {:<6} {:<32} {ok}",
            cap.label(),
            endpoint.provider.label(),
            endpoint.mode.label(),
            endpoint.model
        );
        println!("        {}", endpoint.base_url);
    }

    println!();
    println!("密钥（{}）", AppSettings::config_path().display());
    for id in ProviderId::ALL {
        for mode in EndpointMode::ALL {
            let present = credentials.has(id, mode);
            println!(
                "  {:<8} {:<6} {}",
                id.label(),
                mode.label(),
                if present { "已配置" } else { "—" }
            );
        }
    }
}

/// Renders engine events as terminal output with a live progress bar.
struct CliSink {
    bar: Mutex<ProgressBar>,
}

impl CliSink {
    fn new() -> Self {
        Self {
            bar: Mutex::new(ProgressBar::hidden()),
        }
    }
}

impl EventSink for CliSink {
    fn emit(&self, event: StageEvent) {
        let bar = self.bar.lock().expect("progress bar lock");
        match event {
            StageEvent::Started { label } => {
                let pb = ProgressBar::new_spinner();
                pb.set_style(
                    ProgressStyle::with_template("{spinner} {msg}")
                        .unwrap_or_else(|_| ProgressStyle::default_spinner()),
                );
                pb.enable_steady_tick(Duration::from_millis(120));
                pb.set_message(label.clone());
                drop(bar);
                *self.bar.lock().expect("progress bar lock") = pb;
                println!("▶ {label}");
            }
            StageEvent::Log { level, message } => {
                let prefix = match level {
                    Level::Info => "·",
                    Level::Warn => "!",
                    Level::Error => "✗",
                };
                bar.suspend(|| println!("  {prefix} {message}"));
            }
            StageEvent::Progress { done, total, detail } => {
                if total > 0 {
                    bar.set_style(
                        ProgressStyle::with_template("{spinner} [{bar:24}] {pos}/{len} {msg}")
                            .unwrap_or_else(|_| ProgressStyle::default_bar())
                            .progress_chars("=> "),
                    );
                    bar.set_length(total as u64);
                    bar.set_position(done as u64);
                }
                bar.set_message(detail);
            }
            StageEvent::Item { id, status, detail, .. } => {
                use crate::model::ItemStatus;
                if matches!(status, ItemStatus::Done | ItemStatus::Failed) {
                    let glyph = if status == ItemStatus::Failed { "✗" } else { "✓" };
                    bar.suspend(|| println!("  {glyph} {id} {detail}"));
                }
            }
            StageEvent::Artifact { .. } => {}
            StageEvent::Finished { ok, message } => {
                bar.finish_and_clear();
                println!("{} {message}", if ok { "✔" } else { "✖" });
            }
        }
    }
}
