//! adrama — AI 短剧生产工作流。
//!
//! Layering: `model` (data + disk layout) → `providers` (HTTP behind
//! capability traits) → `engine` (stages, gating, jobs) → front-ends (`cli`,
//! `ui`). Dependencies only ever point downwards.

mod cli;
mod engine;
mod model;
mod providers;
mod settings;
mod ui;
mod update;

use anyhow::Result;
use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

use cli::{AlreadyReported, Cli, Command};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Job failures are already on screen with useful context.
            if err.downcast_ref::<AlreadyReported>().is_none() {
                eprintln!("错误：{err:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // `.env` is a convenience for CLI users; keys are read (never written) here.
    dotenvy::dotenv().ok();
    // Remove backups left by a previous self-update (Windows keeps `.old`).
    update::cleanup_leftovers();

    let args = Cli::parse();
    let launch_gui = args.gui || matches!(args.command, None | Some(Command::Gui));
    init_tracing(launch_gui)?;

    if launch_gui {
        return ui::run(args.project);
    }

    tokio::runtime::Runtime::new()?.block_on(cli::run(args))
}

fn init_tracing(gui: bool) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("adrama=info,warn"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);

    if !gui {
        builder.init();
        return Ok(());
    }

    // 图形界面下 stderr 通常没人看得到（双击启动尤其如此），写文件才有据可查。
    match log_file() {
        Some(file) => builder
            .with_ansi(false)
            .with_writer(move || file.try_clone().expect("clone log file"))
            .init(),
        None => builder.with_writer(std::io::stderr).init(),
    }
    Ok(())
}

/// 追加写的日志文件；超过 2 MB 先截断，免得无限长。
fn log_file() -> Option<std::fs::File> {
    let path = settings::AppSettings::log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    if std::fs::metadata(&path).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}
