//! 配音：把每个镜头的台词合成为语音（云端 TTS 或本地 Piper）。
//!
//! 不是门控阶段——拆解通过后随时可以生成；成片时可选择把配音混入音轨。

use anyhow::{bail, Context, Result};

use super::{StageCtx, StageReport};
use crate::model::{ItemStatus, Project, VoiceMeta};
use crate::providers::SpeechRequest;

#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// 空 = 所有有台词的镜头。
    pub shots: Vec<String>,
    pub force: bool,
}

impl Selection {
    pub fn only(shots: Vec<String>) -> Self {
        Self { shots, force: true }
    }
}

pub async fn run(ctx: &StageCtx<'_>, sel: &Selection) -> Result<StageReport> {
    let bd = ctx.project.load_breakdown()?;
    let targets: Vec<(String, String)> = bd
        .shots
        .iter()
        .filter(|shot| {
            if sel.shots.is_empty() {
                !shot.dialogue.trim().is_empty()
            } else {
                sel.shots.iter().any(|s| s == &shot.id)
            }
        })
        .map(|shot| (shot.id.clone(), shot.dialogue.trim().to_string()))
        .collect();

    if targets.is_empty() {
        bail!("没有包含台词的镜头（拆解里 dialogue 都为空）");
    }

    let audio = ctx.config().audio.clone();

    if ctx.dry_run {
        for (id, text) in &targets {
            ctx.events.info(format!(
                "[演练] 配音 {id} · {} · 「{}」",
                if audio.local_tts {
                    "本地 Piper".to_string()
                } else {
                    format!("云端 · 音色 {}", audio.voice)
                },
                crate::model::index::truncate(text, 40)
            ));
        }
        return Ok(StageReport {
            skipped: targets.len(),
            ..Default::default()
        });
    }

    std::fs::create_dir_all(ctx.project.voice_dir())?;

    // 两条路：本地 Piper（离线、免额度）或云端 API
    let local = if audio.local_tts {
        let piper = crate::tools::resolve_piper().with_context(|| {
            "未安装本地 TTS 引擎：在「配音与字幕」页点「下载 Piper」，或关闭「本地合成」改用云端"
        })?;
        let model = crate::tools::resolve_piper_voice()
            .with_context(|| "未安装音色模型：在「配音与字幕」页点「下载中文音色」")?;
        ctx.events.info(format!(
            "本地合成：Piper · 音色 {}",
            model.file_stem().unwrap_or_default().to_string_lossy()
        ));
        Some((piper.path, model))
    } else {
        None
    };
    let provider = if local.is_none() {
        let p = ctx.factory().speech()?;
        ctx.events.info(format!(
            "云端合成：{} · 音色 {}",
            p.endpoint(),
            audio.voice
        ));
        Some(p)
    } else {
        None
    };

    let bulk = sel.shots.is_empty();
    let total = targets.len() as u32;
    let mut report = StageReport::default();

    for (done, (id, text)) in targets.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.events
            .progress(done as u32, total, format!("{}/{} · {id}", done + 1, total));

        if text.is_empty() {
            report.skipped += 1;
            continue;
        }
        let meta = read_meta(ctx.project, id);
        if bulk && meta.as_ref().map(|m| m.manual).unwrap_or(false) {
            report.skipped += 1;
            ctx.events.info(format!("{id}：手动导入的配音，已保留"));
            continue;
        }
        if ctx.project.find_voice_clip(id).is_some() && !sel.force {
            report.skipped += 1;
            continue;
        }

        let result: Result<(std::path::PathBuf, String, String)> = match (&local, &provider) {
            (Some((piper, model)), _) => {
                let out = ctx.project.voice_dir().join(format!("{id}.wav"));
                let voice_name = model
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let piper = piper.clone();
                let model = model.clone();
                let text_owned = text.clone();
                let out_clone = out.clone();
                // Piper 是同步子进程，放到阻塞线程池，别占住异步执行器
                tokio::task::spawn_blocking(move || {
                    crate::tools::piper_synthesize(&piper, &model, &text_owned, &out_clone)
                })
                .await
                .context("本地合成线程异常")?
                .map(|()| (out, "piper".to_string(), voice_name))
            }
            (None, Some(provider)) => {
                let bytes = provider
                    .synthesize(SpeechRequest {
                        text,
                        voice: &audio.voice,
                    })
                    .await;
                match bytes {
                    Ok(bytes) => {
                        let out = ctx.project.voice_clip(id);
                        tokio::fs::write(&out, &bytes)
                            .await
                            .with_context(|| format!("写入 {}", out.display()))?;
                        Ok((
                            out,
                            provider.endpoint().model.clone(),
                            audio.voice.clone(),
                        ))
                    }
                    Err(err) => Err(err),
                }
            }
            _ => unreachable!("local 与 provider 必有其一"),
        };

        match result {
            Ok((path, model, voice)) => {
                ctx.events.artifact(&path);
                write_meta(
                    ctx.project,
                    id,
                    text,
                    &voice,
                    &model,
                    ItemStatus::Done,
                    None,
                );
                report.generated += 1;
            }
            Err(err) => {
                let msg = format!("{err:#}");
                ctx.events.error(format!("{id}：{msg}"));
                write_meta(
                    ctx.project,
                    id,
                    text,
                    &audio.voice,
                    "",
                    ItemStatus::Failed,
                    Some(msg),
                );
                report.failed += 1;
            }
        }
    }

    ctx.check_cancel()?;
    Ok(report)
}

fn read_meta(project: &Project, id: &str) -> Option<VoiceMeta> {
    let text = std::fs::read_to_string(project.voice_meta(id)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_meta(
    project: &Project,
    id: &str,
    text: &str,
    voice: &str,
    model: &str,
    status: ItemStatus,
    error: Option<String>,
) {
    let manual = read_meta(project, id).map(|m| m.manual).unwrap_or(false);
    let meta = VoiceMeta {
        shot_id: id.to_string(),
        text: text.to_string(),
        voice: voice.to_string(),
        model: model.to_string(),
        status,
        error,
        manual,
    };
    if let Ok(json) = serde_json::to_string_pretty(&meta) {
        let _ = crate::model::project::write_atomic(&project.voice_meta(id), json.as_bytes());
    }
}
