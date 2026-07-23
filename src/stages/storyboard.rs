use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use tracing::info;

use crate::api::openai::OpenAiClient;
use crate::model::{ItemStatus, StoryboardMeta};
use crate::pipeline;
use crate::project::{Project, Stage, StageStatus};
use crate::stages::assets::references_for_shot;

pub async fn run(
    proj: &mut Project,
    scene: Option<u32>,
    shot_id: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let bd = proj.load_breakdown()?;
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
        pipeline::mark(proj, Stage::Storyboard, StageStatus::InProgress)?;
    }

    let size = proj.image_size();
    let style = &proj.config.style;
    let mut generated = 0usize;

    for shot in &bd.shots {
        if let Some(sid) = shot_id {
            if shot.id != sid {
                continue;
            }
        }
        if let Some(sn) = scene {
            let scene_ok = bd
                .scenes
                .iter()
                .find(|s| s.id == shot.scene_id)
                .map(|s| s.number == sn)
                .unwrap_or(false);
            if !scene_ok {
                continue;
            }
        }

        generate_shot(proj, client.as_ref(), &bd, shot, style, size, dry_run, false).await?;
        generated += 1;
    }

    if generated == 0 {
        bail!("no matching shots to generate");
    }

    if !dry_run && shot_id.is_none() && scene.is_none() {
        pipeline::mark(proj, Stage::Storyboard, StageStatus::Done)?;
        println!("Storyboard done under storyboard/. Review then: adrama approve storyboard");
    } else if !dry_run {
        println!("Storyboard shot(s) updated ({generated}).");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn generate_shot(
    proj: &Project,
    client: Option<&OpenAiClient>,
    bd: &crate::model::Breakdown,
    shot: &crate::model::Shot,
    style: &str,
    size: &str,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let out_png = proj.storyboard_dir().join(format!("{}.png", shot.id));
    let out_json = proj.storyboard_dir().join(format!("{}.json", shot.id));

    // Character appearance redundancy for consistency
    let mut char_desc = String::new();
    for cid in &shot.character_ids {
        if let Some(ch) = bd.characters.iter().find(|c| &c.id == cid) {
            char_desc.push_str(&format!(
                " Character {}: {} wearing {}. ",
                ch.name, ch.appearance, ch.costume
            ));
        }
    }

    let prompt = format!(
        "{style}, still frame from a short film, aspect consistent, no text no watermark. \
         Framing: {}. Camera: {}. Action/visual: {}.{} Dialogue context: {}. SFX: {}.",
        shot.framing,
        shot.camera,
        shot.visual,
        char_desc,
        shot.dialogue,
        shot.sfx
    );

    let refs = references_for_shot(proj, bd, shot);
    let ref_paths: Vec<PathBuf> = refs.clone();
    let ref_labels: Vec<String> = ref_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    if dry_run {
        println!("=== DRY RUN storyboard {} ===", shot.id);
        println!("prompt:\n{prompt}");
        println!("references: {ref_labels:?}");
        println!("-> {}", out_png.display());
        return Ok(());
    }

    if out_png.exists() && !force {
        info!("skip existing {}", out_png.display());
        return Ok(());
    }

    info!(
        "generating storyboard {} ({} refs)",
        shot.id,
        ref_paths.len()
    );
    let client = client.expect("client required");
    let ref_refs: Vec<&std::path::Path> = ref_paths.iter().map(|p| p.as_path()).collect();
    client
        .edit_image(&prompt, size, &ref_refs, &out_png)
        .await
        .with_context(|| format!("storyboard image for {}", shot.id))?;

    let meta = StoryboardMeta {
        shot_id: shot.id.clone(),
        prompt,
        reference_assets: ref_labels,
        image: Some(out_png.file_name().unwrap().to_string_lossy().into()),
        status: ItemStatus::Done,
    };
    fs::write(&out_json, serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

pub async fn redo_one(proj: &mut Project, id: &str, dry_run: bool) -> Result<()> {
    let bd = proj.load_breakdown()?;
    let shot = bd
        .shots
        .iter()
        .find(|s| s.id == id)
        .with_context(|| format!("shot not found: {id}"))?;

    let png = proj.storyboard_dir().join(format!("{}.png", shot.id));
    if png.exists() && !dry_run {
        fs::remove_file(&png)?;
    }

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

    generate_shot(
        proj,
        client.as_ref(),
        &bd,
        shot,
        &proj.config.style,
        proj.image_size(),
        dry_run,
        true,
    )
    .await?;
    println!("Redid storyboard {id}");
    Ok(())
}
