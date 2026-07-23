use anyhow::{bail, Context, Result};
use std::fs;
use std::process::Command;
use std::time::Duration;
use tracing::info;

use crate::api::google::GoogleClient;
use crate::model::{ItemStatus, VideoMeta};
use crate::pipeline;
use crate::project::{Project, Stage, StageStatus};

pub async fn run(
    proj: &mut Project,
    shot_id: Option<&str>,
    concat: bool,
    dry_run: bool,
) -> Result<()> {
    let bd = proj.load_breakdown()?;
    let client = if dry_run {
        None
    } else {
        {
            let ep = proj.resolve_video()?;
            Some(GoogleClient::new(ep.api_key, &ep.base_url, &ep.model)?)
        }
    };

    if !dry_run {
        pipeline::mark(proj, Stage::Video, StageStatus::InProgress)?;
    }

    let mut generated = 0usize;
    for shot in &bd.shots {
        if let Some(sid) = shot_id {
            if shot.id != sid {
                continue;
            }
        }
        generate_shot(proj, client.as_ref(), shot, dry_run, false).await?;
        generated += 1;
    }

    if generated == 0 {
        bail!("no matching shots to generate");
    }

    if concat && !dry_run {
        concat_all(proj, &bd)?;
    }

    if !dry_run && shot_id.is_none() {
        pipeline::mark(proj, Stage::Video, StageStatus::Done)?;
        println!("视频片段已写入 video/。请审核后执行: adrama approve video");
    } else if !dry_run {
        println!("已更新 {generated} 个视频镜头。");
    }
    Ok(())
}

async fn generate_shot(
    proj: &Project,
    client: Option<&GoogleClient>,
    shot: &crate::model::Shot,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let sb = proj.storyboard_dir().join(format!("{}.png", shot.id));
    let out_mp4 = proj.video_dir().join(format!("{}.mp4", shot.id));
    let out_json = proj.video_dir().join(format!("{}.json", shot.id));

    let prompt = format!(
        "Cinematic short film clip. Camera: {}. Action: {}. \
         Dialogue (lip-sync if applicable): {}. Ambient/SFX: {}. \
         Keep character appearance consistent with the reference frame. No text overlays.",
        shot.camera, shot.visual, shot.dialogue, shot.sfx
    );

    if dry_run {
        println!("=== DRY RUN video {} ===", shot.id);
        println!("source: {}", sb.display());
        println!("prompt:\n{prompt}");
        println!(
            "aspect: {} duration: {}s",
            proj.config.aspect, shot.duration_secs
        );
        println!("-> {}", out_mp4.display());
        return Ok(());
    }

    if !sb.exists() {
        bail!(
            "storyboard frame missing for {}: {}",
            shot.id,
            sb.display()
        );
    }

    // Resume existing operation if present
    if out_json.exists() && !force {
        if let Ok(meta) = load_meta(&out_json) {
            if out_mp4.exists() && meta.status == ItemStatus::Done {
                info!("skip finished video {}", shot.id);
                return Ok(());
            }
            if let Some(op) = &meta.operation_name {
                info!("resuming operation {op} for {}", shot.id);
                let client = client.expect("client");
                client
                    .wait_and_download(
                        op,
                        &out_mp4,
                        Duration::from_secs(10),
                        Duration::from_secs(60 * 30),
                    )
                    .await?;
                let mut meta = meta;
                meta.video = Some(out_mp4.file_name().unwrap().to_string_lossy().into());
                meta.status = ItemStatus::Done;
                fs::write(&out_json, serde_json::to_string_pretty(&meta)?)?;
                return Ok(());
            }
        }
    }

    if out_mp4.exists() && !force {
        info!("skip existing {}", out_mp4.display());
        return Ok(());
    }

    let client = client.expect("client required");
    info!("submitting video job for {}", shot.id);
    let job = client
        .start_image_to_video(
            &prompt,
            &sb,
            &proj.config.aspect,
            shot.duration_secs,
        )
        .await?;

    let mut meta = VideoMeta {
        shot_id: shot.id.clone(),
        prompt: prompt.clone(),
        source_image: Some(sb.display().to_string()),
        operation_name: Some(job.operation_name.clone()),
        video: None,
        status: ItemStatus::Generating,
    };
    fs::write(&out_json, serde_json::to_string_pretty(&meta)?)?;

    client
        .wait_and_download(
            &job.operation_name,
            &out_mp4,
            Duration::from_secs(10),
            Duration::from_secs(60 * 30),
        )
        .await
        .with_context(|| format!("wait/download video for {}", shot.id))?;

    meta.video = Some(out_mp4.file_name().unwrap().to_string_lossy().into());
    meta.status = ItemStatus::Done;
    fs::write(&out_json, serde_json::to_string_pretty(&meta)?)?;
    println!("Saved {}", out_mp4.display());
    Ok(())
}

fn load_meta(path: &std::path::Path) -> Result<VideoMeta> {
    let s = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&s)?)
}

fn concat_all(proj: &Project, bd: &crate::model::Breakdown) -> Result<()> {
    let list_path = proj.video_dir().join("concat_list.txt");
    let mut lines = Vec::new();
    for shot in &bd.shots {
        let mp4 = proj.video_dir().join(format!("{}.mp4", shot.id));
        if mp4.exists() {
            // ffmpeg concat demuxer needs escaped single quotes
            let p = mp4.display().to_string().replace('\'', "'\\''");
            lines.push(format!("file '{p}'"));
        }
    }
    if lines.is_empty() {
        bail!("no mp4 clips to concatenate");
    }
    fs::write(&list_path, lines.join("\n") + "\n")?;

    let out = proj.video_dir().join("final.mp4");
    info!("concatenating {} clips -> {}", lines.len(), out.display());
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list_path.to_str().unwrap(),
            "-c",
            "copy",
            out.to_str().unwrap(),
        ])
        .status()
        .context("ffmpeg not found; install ffmpeg or skip --concat")?;

    if !status.success() {
        bail!("ffmpeg concat failed with {status}");
    }
    println!("成片: {}", out.display());
    Ok(())
}

pub async fn redo_one(proj: &mut Project, id: &str, dry_run: bool) -> Result<()> {
    let bd = proj.load_breakdown()?;
    let shot = bd
        .shots
        .iter()
        .find(|s| s.id == id)
        .with_context(|| format!("shot not found: {id}"))?;

    let mp4 = proj.video_dir().join(format!("{}.mp4", shot.id));
    let json = proj.video_dir().join(format!("{}.json", shot.id));
    if !dry_run {
        if mp4.exists() {
            fs::remove_file(&mp4)?;
        }
        if json.exists() {
            fs::remove_file(&json)?;
        }
    }

    let client = if dry_run {
        None
    } else {
        {
            let ep = proj.resolve_video()?;
            Some(GoogleClient::new(ep.api_key, &ep.base_url, &ep.model)?)
        }
    };

    generate_shot(proj, client.as_ref(), shot, dry_run, true).await?;
    Ok(())
}
