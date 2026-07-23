use anyhow::{bail, Result};

use crate::project::{Project, Stage};
use crate::stages::{assets, parse, storyboard, video};

pub async fn run(proj: &mut Project, stage: Stage, id: &str, dry_run: bool) -> Result<()> {
    match stage {
        Stage::Parse => {
            // re-run full parse (id ignored)
            let _ = id;
            parse::run(proj, dry_run).await?;
        }
        Stage::Assets => {
            assets::redo_one(proj, id, dry_run).await?;
        }
        Stage::Storyboard => {
            storyboard::redo_one(proj, id, dry_run).await?;
        }
        Stage::Video => {
            video::redo_one(proj, id, dry_run).await?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn validate_id(stage: Stage, id: &str) -> Result<()> {
    if id.is_empty() {
        bail!("--id is required for redo on stage {stage}");
    }
    Ok(())
}
