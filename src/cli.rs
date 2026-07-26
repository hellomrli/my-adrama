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
    AspectRatio, Capability, Project, ProjectIndex, Stage,
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
    /// 测试某种能力当前配置的端点是否可用
    Test {
        /// chat / image / video
        capability: String,
    },
    /// 用 AI 把剧本整理成标准影视剧本模板（原稿备份为 .bak）
    Format {
        #[arg(long)]
        dry_run: bool,
    },
    /// 逐镜头配音（云端 TTS 或本地 Piper，按 project.toml 的 audio 设置）
    Voice {
        #[arg(long = "shot", value_name = "ID")]
        shots: Vec<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// 生成 SRT 字幕（video/subtitles.srt）
    Subtitles,
    /// 把项目打包成 .adrama.tar.gz（迁移到另一台设备）
    Pack {
        /// 输出目录（默认当前目录）
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// 解开项目包
    Unpack {
        /// 项目包路径
        archive: PathBuf,
        /// 解压到哪个目录（默认当前目录）
        #[arg(long, default_value = ".")]
        to: PathBuf,
    },
    /// 查看/安装本地工具：ffmpeg、Piper（本地 TTS）、中文音色
    Tools {
        /// 安装某个工具：ffmpeg / piper / voice
        #[arg(long)]
        install: Option<String>,
    },
    /// 检查（或安装）新版本
    Update {
        /// 下载并安装，而不只是检查
        #[arg(long)]
        apply: bool,
    },
    /// 列出某种能力所配端点当前提供的模型
    Models {
        /// chat / image / video
        capability: String,
        /// 列出全部模型，而不只是与该能力相关的
        #[arg(long)]
        all: bool,
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
        Command::Test { capability } => {
            let cap: Capability = capability.parse()?;
            let request = probe_request(&cli.project, cap, &settings)?;
            dispatch(&cli.project, Job::Probe(request), false, &settings).await
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
        Command::Format { dry_run } => {
            dispatch(&cli.project, Job::FormatScript, dry_run, &settings).await
        }
        Command::Voice {
            shots,
            force,
            dry_run,
        } => {
            let job = Job::Voice(engine::stages::voice::Selection {
                force: force || !shots.is_empty(),
                shots,
            });
            dispatch(&cli.project, job, dry_run, &settings).await
        }
        Command::Subtitles => {
            let (path, count) = engine::write_subtitles(&cli.project)?;
            println!("已生成 {count} 条字幕 → {}", path.display());
            Ok(())
        }
        Command::Pack { out } => {
            let archive = engine::pack_project(&cli.project, &out)?;
            println!("已打包 → {}", archive.display());
            println!("（密钥不随包走：新设备上在设置里重新配置，或复制本机的 settings.json）");
            Ok(())
        }
        Command::Unpack { archive, to } => {
            let root = engine::unpack_project(&archive, &to)?;
            println!("已导入 → {}", root.display());
            println!("下一步：adrama --project \"{}\" status", root.display());
            Ok(())
        }
        Command::Tools { install } => tools_command(install.as_deref()).await,
        Command::Update { apply } => update_command(apply).await,
        Command::Models { capability, all } => {
            models_command(&cli.project, &capability, all, &settings).await
        }
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

async fn tools_command(install: Option<&str>) -> Result<()> {
    if let Some(name) = install {
        let tool = match name.to_ascii_lowercase().as_str() {
            "ffmpeg" => crate::tools::Tool::Ffmpeg,
            "piper" => crate::tools::Tool::Piper,
            "voice" | "音色" => crate::tools::Tool::PiperVoice,
            other => anyhow::bail!("未知工具：{other}（可选 ffmpeg / piper / voice）"),
        };
        let bar = ProgressBar::new(0);
        bar.set_style(
            ProgressStyle::with_template("{spinner} [{bar:24}] {bytes}/{total_bytes} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=> "),
        );
        bar.set_message(format!("下载 {}", tool.label()));
        let msg = crate::tools::install(tool, |received, total| {
            bar.set_length(total.max(received));
            bar.set_position(received);
        })
        .await?;
        bar.finish_and_clear();
        println!("✔ {msg}");
        return Ok(());
    }

    println!("本地工具（托管目录 {}）", crate::tools::tools_dir().display());
    match crate::tools::resolve_ffmpeg() {
        Some(s) => println!(
            "  ffmpeg   {}（{}）  {}",
            s.version,
            if s.managed { "托管" } else { "系统" },
            s.path.display()
        ),
        None => println!("  ffmpeg   未安装  → adrama tools --install ffmpeg"),
    }
    match crate::tools::resolve_piper() {
        Some(s) => println!(
            "  piper    {}（{}）  {}",
            s.version,
            if s.managed { "托管" } else { "系统" },
            s.path.display()
        ),
        None => println!("  piper    未安装  → adrama tools --install piper"),
    }
    match crate::tools::resolve_piper_voice() {
        Some(p) => println!("  音色     {}", p.display()),
        None => println!("  音色     未安装  → adrama tools --install voice"),
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

/// 按能力组装一次探测请求：端点来自项目配置，密钥来自本机设置。
fn probe_request(project_dir: &Path, cap: Capability, settings: &AppSettings) -> Result<ProbeRequest> {
    let config = Project::open(project_dir)
        .map(|p| p.config)
        .unwrap_or_default();
    let endpoint = config.endpoint(cap);
    let api_key = settings
        .credentials()
        .get(cap, endpoint.provider, endpoint.mode)
        .map(str::to_string)
        .with_context(|| {
            format!(
                "「{}」还没有配置 {} 的{}密钥",
                cap.label(),
                endpoint.provider.label(),
                endpoint.mode.label()
            )
        })?;

    Ok(ProbeRequest {
        capability: cap,
        provider: endpoint.provider,
        mode: endpoint.mode,
        base_url: endpoint.base_url,
        api_key,
        model: endpoint.model,
    })
}

async fn models_command(
    project_dir: &Path,
    capability: &str,
    all: bool,
    settings: &AppSettings,
) -> Result<()> {
    let cap: Capability = capability.parse()?;
    let request = probe_request(project_dir, cap, settings)?;
    let report = crate::providers::probe(
        request.provider,
        request.mode,
        &request.base_url,
        &request.api_key,
        &request.model,
    )
    .await?;

    if report.models.is_empty() {
        println!("该端点没有返回模型列表（代理可能未实现 /models），模型 ID 需手动填写");
        return Ok(());
    }

    println!(
        "{} · {} · {} — 共 {} 个模型",
        cap.label(),
        request.provider.label(),
        request.mode.label(),
        report.models.len()
    );
    let mut shown = 0;
    for model in &report.models {
        if all || crate::providers::looks_like(cap, model) {
            let marker = if *model == request.model { "→" } else { " " };
            println!("  {marker} {model}");
            shown += 1;
        }
    }
    if !all && shown < report.models.len() {
        println!();
        println!(
            "（已按名称筛出可能用于「{}」的 {shown} 个；加 --all 看全部 {}）",
            cap.label(),
            report.models.len()
        );
    }
    Ok(())
}

fn parse_stage(s: &str) -> Result<Stage> {
    s.parse()
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
    println!("三种能力各自独立配置（{}）", AppSettings::config_path().display());
    for cap in Capability::ALL {
        let endpoint = project.config.endpoint(cap);
        let ok = if !endpoint.provider.supports(cap) {
            "✗ 该服务商不提供此能力"
        } else if credentials.has(cap, endpoint.provider, endpoint.mode) {
            "✓"
        } else {
            "! 缺少密钥"
        };
        println!();
        println!("{} {ok}", cap.label());
        println!("  服务商  {} · {}", endpoint.provider.label(), endpoint.mode.label());
        println!("  地址    {}", endpoint.base_url);
        println!("  模型    {}", endpoint.model);
        let cached = settings.known_models(cap, endpoint.provider, endpoint.mode).len();
        if cached > 0 {
            println!("  已缓存  {cached} 个可选模型");
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
