use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::api::openai::OpenAiClient;
use crate::model::{AssetMeta, Breakdown, ItemStatus};
use crate::pipeline;
use crate::project::{Project, Stage, StageStatus};

const CHAR_VIEWS: &[(&str, &str)] = &[
    ("front", "front view portrait, looking at camera, neutral expression"),
    ("side", "side profile view, looking left"),
    ("full", "full body standing, front view, head to toe"),
];

pub async fn run(proj: &mut Project, only: Option<&str>, dry_run: bool) -> Result<()> {
    let breakdown = proj.load_breakdown()?;
    let client = if dry_run {
        None
    } else {
        {
            let ep = proj.resolve_image()?;
            Some(OpenAiClient::new(
                ep.api_key,
                &ep.base_url,
                &proj.config.openai_chat_model,
                &ep.model,
            )?)
        }
    };

    if !dry_run {
        pipeline::mark(proj, Stage::Assets, StageStatus::InProgress)?;
    }

    let size = proj.image_size();
    let style = &proj.config.style;

    // Characters
    for ch in &breakdown.characters {
        if let Some(only) = only {
            if ch.name != only && ch.id != only {
                continue;
            }
        }
        generate_character(proj, client.as_ref(), ch, style, size, dry_run).await?;
    }

    // Costumes
    for c in &breakdown.costumes {
        if let Some(only) = only {
            if c.name != only && c.id != only {
                continue;
            }
        }
        let prompt = format!(
            "{}, costume reference sheet, isolated on neutral gray background, \
             full detail of fabric and accessories. Description: {}",
            style, c.description
        );
        gen_single(
            proj,
            client.as_ref(),
            "costumes",
            &c.id,
            &c.name,
            &prompt,
            size,
            dry_run,
        )
        .await?;
    }

    // Props
    for p in &breakdown.props {
        if let Some(only) = only {
            if p.name != only && p.id != only {
                continue;
            }
        }
        let prompt = format!(
            "{}, product-style prop reference photo, isolated on neutral background. \
             Description: {}",
            style, p.description
        );
        gen_single(
            proj,
            client.as_ref(),
            "props",
            &p.id,
            &p.name,
            &prompt,
            size,
            dry_run,
        )
        .await?;
    }

    // Locations
    for loc in &breakdown.locations {
        if let Some(only) = only {
            if loc.name != only && loc.id != only {
                continue;
            }
        }
        let prompt = format!(
            "{}, establishing shot of location, empty of people, \
             time of day: {}. Description: {}",
            style, loc.time_of_day, loc.description
        );
        gen_single(
            proj,
            client.as_ref(),
            "locations",
            &loc.id,
            &loc.name,
            &prompt,
            size,
            dry_run,
        )
        .await?;
    }

    // If only was set and nothing matched costumes/props/locations/characters from
    // partial filter — that's fine.

    if !dry_run && only.is_none() {
        pipeline::mark(proj, Stage::Assets, StageStatus::Done)?;
        println!("Assets generated under assets/. Review then: adrama approve assets");
    } else if !dry_run {
        // partial redo — leave stage as Done if it was already
        if proj.state.get(Stage::Assets) == StageStatus::Pending {
            pipeline::mark(proj, Stage::Assets, StageStatus::Done)?;
        }
        println!("Asset(s) updated.");
    }

    Ok(())
}

async fn generate_character(
    proj: &Project,
    client: Option<&OpenAiClient>,
    ch: &crate::model::Character,
    style: &str,
    size: &str,
    dry_run: bool,
) -> Result<()> {
    let dir = proj.assets_dir().join("characters").join(&ch.id);
    fs::create_dir_all(&dir)?;

    let base_prompt = format!(
        "{}, character design reference, consistent face and identity. \
         Name: {}. Appearance: {}. Costume: {}. Personality vibe: {}.",
        style, ch.name, ch.appearance, ch.costume, ch.personality
    );

    let mut files = Vec::new();
    for (view, suffix) in CHAR_VIEWS {
        let prompt = format!("{base_prompt} View: {suffix}. No text, no watermark.");
        let filename = format!("{view}.png");
        let out = dir.join(&filename);

        if dry_run {
            println!("=== DRY RUN character {} / {} ===", ch.id, view);
            println!("{prompt}\n-> {}", out.display());
            continue;
        }

        // Skip if already exists (unless redoing via overwrite from redo)
        if out.exists() {
            info!("skip existing {}", out.display());
            files.push(filename);
            continue;
        }

        info!("generating character {} ({view})", ch.name);
        let client = client.expect("client required when not dry_run");
        client.generate_image(&prompt, size, &out).await?;
        files.push(filename);
    }

    if dry_run {
        return Ok(());
    }

    // Save base prompt for redo
    fs::write(dir.join("prompt.txt"), &base_prompt)?;
    let meta = AssetMeta {
        id: ch.id.clone(),
        kind: "character".into(),
        name: ch.name.clone(),
        prompt: base_prompt,
        files,
        status: ItemStatus::Done,
    };
    fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn gen_single(
    proj: &Project,
    client: Option<&OpenAiClient>,
    kind: &str,
    id: &str,
    name: &str,
    prompt: &str,
    size: &str,
    dry_run: bool,
) -> Result<()> {
    let dir = proj.assets_dir().join(kind).join(id);
    fs::create_dir_all(&dir)?;
    let out = dir.join("ref.png");

    if dry_run {
        println!("=== DRY RUN {kind}/{id} ===");
        println!("{prompt}\n-> {}", out.display());
        return Ok(());
    }

    if out.exists() {
        info!("skip existing {}", out.display());
    } else {
        info!("generating {kind}: {name}");
        let client = client.expect("client required when not dry_run");
        client.generate_image(prompt, size, &out).await?;
    }

    fs::write(dir.join("prompt.txt"), prompt)?;
    let meta = AssetMeta {
        id: id.into(),
        kind: kind.into(),
        name: name.into(),
        prompt: prompt.into(),
        files: vec!["ref.png".into()],
        status: ItemStatus::Done,
    };
    fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

/// Force regenerate one asset (delete images first).
pub async fn redo_one(proj: &mut Project, id: &str, dry_run: bool) -> Result<()> {
    // Find asset dir by id under characters/costumes/props/locations
    if let Some(dir) = find_asset_dir(proj, id)? {
        // remove generated images so generate will not skip
        clear_images(&dir)?;
        // re-read prompt or rebuild from breakdown
        return run(proj, Some(id), dry_run).await;
    }

    // Also match character by name via breakdown
    let bd = proj.load_breakdown()?;
    if let Some(ch) = bd
        .characters
        .iter()
        .find(|c| c.id == id || c.name == id)
    {
        let dir = proj.assets_dir().join("characters").join(&ch.id);
        if dir.exists() {
            clear_images(&dir)?;
        }
        return run(proj, Some(&ch.id), dry_run).await;
    }

    anyhow::bail!("asset id not found: {id}");
}

fn find_asset_dir(proj: &Project, id: &str) -> Result<Option<PathBuf>> {
    for kind in ["characters", "costumes", "props", "locations"] {
        let dir = proj.assets_dir().join(kind).join(id);
        if dir.exists() {
            return Ok(Some(dir));
        }
    }
    Ok(None)
}

fn clear_images(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e, "png" | "jpg" | "jpeg" | "webp"))
            .unwrap_or(false)
        {
            fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    Ok(())
}

/// Collect reference image paths for a shot from breakdown + assets on disk.
pub fn references_for_shot(proj: &Project, bd: &Breakdown, shot: &crate::model::Shot) -> Vec<PathBuf> {
    let mut refs = Vec::new();

    for cid in &shot.character_ids {
        let front = proj
            .assets_dir()
            .join("characters")
            .join(cid)
            .join("front.png");
        if front.exists() {
            refs.push(front);
        } else {
            // try any png in character dir
            let dir = proj.assets_dir().join("characters").join(cid);
            if let Ok(rd) = fs::read_dir(dir) {
                for e in rd.flatten() {
                    if e.path().extension().and_then(|x| x.to_str()) == Some("png") {
                        refs.push(e.path());
                        break;
                    }
                }
            }
        }
    }

    let loc_id = shot
        .location_id
        .clone()
        .or_else(|| {
            bd.scenes
                .iter()
                .find(|s| s.id == shot.scene_id)
                .and_then(|s| s.location_id.clone())
        });
    if let Some(lid) = loc_id {
        let p = proj.assets_dir().join("locations").join(lid).join("ref.png");
        if p.exists() {
            refs.push(p);
        }
    }

    for pid in &shot.prop_ids {
        let p = proj.assets_dir().join("props").join(pid).join("ref.png");
        if p.exists() {
            refs.push(p);
        }
    }

    refs
}
