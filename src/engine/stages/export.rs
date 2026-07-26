//! Final cut — concatenate approved clips with ffmpeg.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

use super::StageCtx;
use crate::model::Stage;

pub struct ExportReport {
    pub output: PathBuf,
    pub clips: usize,
}

pub async fn run(ctx: &StageCtx<'_>) -> Result<ExportReport> {
    let bd = ctx.project.load_breakdown()?;
    let dir = ctx.project.video_dir();

    // Relative names, so the list file works regardless of how exotic the
    // project path is (spaces, CJK, Windows drive letters).
    let mut names = Vec::new();
    for shot in &bd.shots {
        let clip = ctx.project.video_clip(&shot.id);
        if clip.is_file() {
            names.push(format!("{}.mp4", shot.id));
        } else {
            ctx.events
                .warn(format!("跳过缺失片段：{}", clip.display()));
        }
    }

    if names.is_empty() {
        bail!("video/ 下没有可拼接的片段");
    }

    let output = ctx.project.final_cut();
    if ctx.dry_run {
        ctx.events.info(format!(
            "[演练] 将按顺序拼接 {} 个片段 → {}\n{}",
            names.len(),
            output.display(),
            names.join("\n")
        ));
        return Ok(ExportReport {
            output,
            clips: names.len(),
        });
    }

    let list_path = dir.join("concat_list.txt");
    let list_body = names
        .iter()
        .map(|n| format!("file '{}'", n.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&list_path, format!("{list_body}\n"))
        .with_context(|| format!("写入 {}", list_path.display()))?;

    ctx.events
        .info(format!("ffmpeg 拼接 {} 个片段…", names.len()));
    ctx.events.progress(0, 1, "ffmpeg 运行中");
    ctx.check_cancel()?;

    let status = tokio::process::Command::new("ffmpeg")
        .current_dir(&dir)
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            "concat_list.txt",
            "-c",
            "copy",
            "final.mp4",
        ])
        .status()
        .await
        .context("未找到 ffmpeg，请先安装（Debian/Ubuntu: sudo apt install ffmpeg）")?;

    if !status.success() {
        bail!("ffmpeg 拼接失败（退出码 {status}）；若片段编码不一致，可改用重编码方式拼接");
    }

    ctx.events.artifact(&output);
    ctx.events.progress(1, 1, "拼接完成");
    ctx.events
        .item(Stage::Video, "final", crate::model::ItemStatus::Done, "成片已生成");

    Ok(ExportReport {
        output,
        clips: names.len(),
    })
}
