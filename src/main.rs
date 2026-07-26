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
    if gui {
        // Keep stdout clean for the desktop app's parent shell.
        builder.with_writer(std::io::stderr).init();
    } else {
        builder.init();
    }
    Ok(())
}
