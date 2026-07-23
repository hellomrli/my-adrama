use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

use crate::project::Project;

pub fn run(proj: &Project, script: &Path) -> Result<()> {
    if !script.exists() {
        bail!("script not found: {}", script.display());
    }
    let dest_dir = proj.script_dir();
    fs::create_dir_all(&dest_dir)?;

    let name = script
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("script.md");
    let dest = dest_dir.join(name);
    fs::copy(script, &dest)
        .with_context(|| format!("copy {} -> {}", script.display(), dest.display()))?;
    println!("Imported script -> {}", dest.display());
    Ok(())
}
