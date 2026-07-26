//! 成片：拼接片段，可选混入配音、烧录字幕。
//!
//! ffmpeg 从托管目录或系统 PATH 解析；全部命令都以 video/ 为工作目录、
//! 用相对文件名——Windows 上 subtitles 滤镜的路径转义是出了名的坑，
//! 相对路径直接绕开它。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use super::StageCtx;
use crate::model::Stage;

pub struct ExportReport {
    pub output: PathBuf,
    pub clips: usize,
    pub mixed_voice: bool,
    pub burned_subs: bool,
}

pub async fn run(ctx: &StageCtx<'_>) -> Result<ExportReport> {
    let bd = ctx.project.load_breakdown()?;
    let dir = ctx.project.video_dir();
    let audio = ctx.config().audio.clone();

    // 收集存在的片段（保持镜头顺序）
    let mut names: Vec<(String, u32)> = Vec::new();
    for shot in &bd.shots {
        if ctx.project.video_clip(&shot.id).is_file() {
            names.push((format!("{}.mp4", shot.id), shot.duration_secs));
        } else {
            ctx.events
                .warn(format!("跳过缺失片段：{}.mp4", shot.id));
        }
    }
    if names.is_empty() {
        bail!("video/ 下没有可拼接的片段");
    }

    let output = ctx.project.final_cut();
    if ctx.dry_run {
        ctx.events.info(format!(
            "[演练] 拼接 {} 个片段 → {}{}{}",
            names.len(),
            output.display(),
            if audio.mix_voiceover { "，混入配音" } else { "" },
            if audio.burn_subtitles { "，烧录字幕" } else { "" },
        ));
        return Ok(ExportReport {
            output,
            clips: names.len(),
            mixed_voice: audio.mix_voiceover,
            burned_subs: audio.burn_subtitles,
        });
    }

    let ffmpeg = crate::tools::resolve_ffmpeg().with_context(|| {
        "未找到 ffmpeg：在「配音与字幕」页可一键下载最新版，或自行安装到系统 PATH"
    })?;
    ctx.events.info(format!(
        "ffmpeg：{}（{}）",
        ffmpeg.version,
        if ffmpeg.managed { "托管" } else { "系统" }
    ));

    let steps = 1 + u32::from(audio.mix_voiceover) + u32::from(audio.burn_subtitles);
    let mut step = 0u32;

    // 1) 拼接
    step += 1;
    ctx.events.progress(step - 1, steps, "拼接片段");
    ctx.check_cancel()?;
    let list_body = names
        .iter()
        .map(|(n, _)| format!("file '{}'", n.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("concat_list.txt"), format!("{list_body}\n"))?;

    let needs_post = audio.mix_voiceover || audio.burn_subtitles;
    let concat_out = if needs_post { "_concat.mp4" } else { "final.mp4" };
    run_ffmpeg(
        &ffmpeg.path,
        &dir,
        &[
            "-y", "-f", "concat", "-safe", "0", "-i", "concat_list.txt", "-c", "copy", concat_out,
        ],
    )
    .await?;
    let mut current = concat_out.to_string();

    // 2) 配音混音：每镜一段（配音补齐/截断到镜头时长，没配音就是静音），
    //    连成整条音轨后替换原声。
    let mut mixed = false;
    if audio.mix_voiceover {
        step += 1;
        ctx.events.progress(step - 1, steps, "合成配音音轨");
        ctx.check_cancel()?;

        let mut seg_names = Vec::new();
        let mut have_any = false;
        for (i, (name, dur)) in names.iter().enumerate() {
            let shot_id = name.trim_end_matches(".mp4");
            let seg = format!("_vo_{i:03}.m4a");
            let dur = dur.to_string();
            match ctx.project.find_voice_clip(shot_id) {
                Some(voice) => {
                    have_any = true;
                    let rel = pathdiff_display(&voice, &dir);
                    run_ffmpeg(
                        &ffmpeg.path,
                        &dir,
                        &[
                            "-y", "-i", &rel, "-af", "apad", "-t", &dur, "-ar", "44100",
                            "-ac", "2", "-c:a", "aac", &seg,
                        ],
                    )
                    .await?;
                }
                None => {
                    run_ffmpeg(
                        &ffmpeg.path,
                        &dir,
                        &[
                            "-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", &dur,
                            "-c:a", "aac", &seg,
                        ],
                    )
                    .await?;
                }
            }
            seg_names.push(seg);
        }

        if have_any {
            let audio_list = seg_names
                .iter()
                .map(|n| format!("file '{n}'"))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(dir.join("_vo_list.txt"), format!("{audio_list}\n"))?;
            run_ffmpeg(
                &ffmpeg.path,
                &dir,
                &[
                    "-y", "-f", "concat", "-safe", "0", "-i", "_vo_list.txt", "-c", "copy",
                    "_voiceover.m4a",
                ],
            )
            .await?;

            let out = if audio.burn_subtitles { "_dub.mp4" } else { "final.mp4" };
            run_ffmpeg(
                &ffmpeg.path,
                &dir,
                &[
                    "-y", "-i", &current, "-i", "_voiceover.m4a", "-map", "0:v", "-map", "1:a",
                    "-c:v", "copy", "-c:a", "copy", "-shortest", out,
                ],
            )
            .await?;
            current = out.to_string();
            mixed = true;
            ctx.events
                .warn("已用配音替换片段原声（不想替换可在「配音与字幕」页关闭混音）");
        } else {
            ctx.events
                .warn("没有任何镜头有配音文件，跳过混音（先在「配音与字幕」页生成配音）");
        }
    }

    // 3) 字幕烧录（需要重编码视频）
    let mut burned = false;
    if audio.burn_subtitles {
        step += 1;
        ctx.events.progress(step - 1, steps, "烧录字幕（重编码）");
        ctx.check_cancel()?;

        let (srt, count) = super::super::subtitles::srt(&bd);
        if count == 0 {
            ctx.events.warn("没有台词，跳过字幕烧录");
            if current != "final.mp4" {
                std::fs::rename(dir.join(&current), dir.join("final.mp4"))?;
            }
        } else {
            std::fs::write(dir.join("subtitles.srt"), srt)?;
            run_ffmpeg(
                &ffmpeg.path,
                &dir,
                &[
                    "-y", "-i", &current, "-vf", "subtitles=subtitles.srt", "-c:a", "copy",
                    "final.mp4",
                ],
            )
            .await?;
            burned = true;
        }
    }

    // 清理中间产物
    for name in std::fs::read_dir(&dir)?.flatten() {
        let file_name = name.file_name().to_string_lossy().to_string();
        if file_name.starts_with("_vo_") || file_name == "_concat.mp4" || file_name == "_dub.mp4"
            || file_name == "_voiceover.m4a"
        {
            let _ = std::fs::remove_file(name.path());
        }
    }

    ctx.events.artifact(&output);
    ctx.events.progress(steps, steps, "成片完成");
    ctx.events
        .item(Stage::Video, "final", crate::model::ItemStatus::Done, "成片已生成");

    Ok(ExportReport {
        output,
        clips: names.len(),
        mixed_voice: mixed,
        burned_subs: burned,
    })
}

async fn run_ffmpeg(ffmpeg: &Path, cwd: &Path, args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new(ffmpeg)
        .current_dir(cwd)
        .args(args)
        .output()
        .await
        .with_context(|| format!("运行 {}", ffmpeg.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
        bail!(
            "ffmpeg 失败（{}）：{}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        );
    }
    Ok(())
}

/// voice/ 相对 video/ 的路径（同一项目内固定是 ../voice/x）。
fn pathdiff_display(target: &Path, base: &Path) -> String {
    match (target.parent(), base.parent()) {
        (Some(tp), Some(bp)) if tp.parent() == Some(bp) || tp == bp => {
            // 常规布局：video/ 与 voice/ 同级
            format!(
                "../{}/{}",
                tp.file_name().unwrap_or_default().to_string_lossy(),
                target.file_name().unwrap_or_default().to_string_lossy()
            )
        }
        _ => target.display().to_string(),
    }
}
