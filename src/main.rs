mod api;
mod gui;
mod model;
mod pipeline;
mod project;
mod settings;
mod stages;
mod util;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use project::{Project, Stage};

#[derive(Parser, Debug)]
#[command(name = "adrama", version, about = "AI short-drama production workflow (CLI + GUI)")]
struct Cli {
    /// Project root directory (default: current directory)
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,

    /// Force open the graphical interface
    #[arg(long, global = true)]
    gui: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new drama project
    New {
        /// Project name / directory
        name: String,
        /// Visual style prefix for image prompts
        #[arg(long, default_value = "cinematic, photorealistic, film grain")]
        style: String,
        /// Aspect ratio: 16:9 or 9:16
        #[arg(long, default_value = "16:9")]
        aspect: String,
    },
    /// Import a script markdown file into the project
    Import {
        /// Path to script file
        script: PathBuf,
    },
    /// Stage 1: parse script into structured breakdown.json
    Parse {
        /// Print prompts without calling APIs
        #[arg(long)]
        dry_run: bool,
    },
    /// Stage 2: generate character / costume / prop / location assets
    Assets {
        /// Only regenerate this asset name
        #[arg(long)]
        only: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Stage 3: generate storyboard frames
    Storyboard {
        /// Only generate for this scene number (1-based)
        #[arg(long)]
        scene: Option<u32>,
        /// Only generate for this shot id
        #[arg(long)]
        shot: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Stage 4: generate video clips from storyboard frames
    Video {
        /// Only generate for this shot id
        #[arg(long)]
        shot: Option<String>,
        /// Concatenate approved clips with ffmpeg after generation
        #[arg(long)]
        concat: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show pipeline stage status
    Status,
    /// Approve a stage, unlocking the next one
    Approve {
        /// Stage name: parse | assets | storyboard | video
        stage: String,
    },
    /// Re-generate a single item in a stage
    Redo {
        /// Stage name: parse | assets | storyboard | video
        stage: String,
        /// Item id (character name, shot id, etc.)
        #[arg(long)]
        id: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Open the graphical interface
    Gui,
}

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    // Load persisted API keys into process env (GUI + CLI)
    {
        let mut s = settings::AppSettings::load();
        s.merge_from_env();
        s.apply_to_env();
    }

    let cli = Cli::parse();

    // No subcommand, or explicit Gui / --gui → desktop UI
    let launch_gui = cli.gui
        || matches!(cli.command, None | Some(Commands::Gui));

    if launch_gui {
        init_tracing_gui()?;
        return gui::run(cli.project);
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(cli))
}

fn init_tracing_gui() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("adrama=info".parse()?))
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

fn init_tracing_cli() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("adrama=info".parse()?))
        .with_target(false)
        .init();
    Ok(())
}

async fn async_main(cli: Cli) -> Result<()> {
    init_tracing_cli()?;

    let command = cli.command.expect("CLI path requires a subcommand");

    match command {
        Commands::New {
            name,
            style,
            aspect,
        } => {
            let path = PathBuf::from(&name);
            let display_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&name);
            Project::create(&path, display_name, &style, &aspect)?;
            println!("Created project at {}", path.display());
        }
        Commands::Import { script } => {
            let proj = Project::open(&cli.project)?;
            stages::import::run(&proj, &script)?;
        }
        Commands::Parse { dry_run } => {
            let mut proj = Project::open(&cli.project)?;
            pipeline::require_stage(&proj, Stage::Parse)?;
            stages::parse::run(&mut proj, dry_run).await?;
        }
        Commands::Assets { only, dry_run } => {
            let mut proj = Project::open(&cli.project)?;
            if !dry_run {
                pipeline::require_approved(&proj, Stage::Parse)?;
            }
            stages::assets::run(&mut proj, only.as_deref(), dry_run).await?;
        }
        Commands::Storyboard {
            scene,
            shot,
            dry_run,
        } => {
            let mut proj = Project::open(&cli.project)?;
            if !dry_run {
                pipeline::require_approved(&proj, Stage::Assets)?;
            }
            stages::storyboard::run(&mut proj, scene, shot.as_deref(), dry_run).await?;
        }
        Commands::Video {
            shot,
            concat,
            dry_run,
        } => {
            let mut proj = Project::open(&cli.project)?;
            if !dry_run {
                pipeline::require_approved(&proj, Stage::Storyboard)?;
            }
            stages::video::run(&mut proj, shot.as_deref(), concat, dry_run).await?;
        }
        Commands::Status => {
            let proj = Project::open(&cli.project)?;
            stages::status::run(&proj)?;
        }
        Commands::Approve { stage } => {
            let mut proj = Project::open(&cli.project)?;
            let stage = parse_stage(&stage)?;
            pipeline::approve(&mut proj, stage)?;
            println!("Approved stage: {stage}");
        }
        Commands::Redo {
            stage,
            id,
            dry_run,
        } => {
            let mut proj = Project::open(&cli.project)?;
            let stage = parse_stage(&stage)?;
            stages::redo::run(&mut proj, stage, &id, dry_run).await?;
        }
        Commands::Gui => unreachable!("handled before async_main"),
    }

    Ok(())
}

fn parse_stage(s: &str) -> Result<Stage> {
    s.parse()
        .with_context(|| format!("unknown stage '{s}', expected parse|assets|storyboard|video"))
}
