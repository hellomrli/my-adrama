//! Stage 1 — screenplay → structured breakdown.

use anyhow::{Context, Result};

use super::StageCtx;
use crate::engine::prompts;
use crate::model::Breakdown;
use crate::providers::ChatJsonRequest;

pub async fn run(ctx: &StageCtx<'_>) -> Result<Breakdown> {
    let (script_path, script) = ctx.project.read_script()?;
    ctx.events
        .info(format!("剧本：{}（{} 字）", script_path.display(), script.chars().count()));

    let user = prompts::parse_user_prompt(ctx.config(), &script);
    let schema = prompts::breakdown_schema();

    if ctx.dry_run {
        ctx.events.info("— 演练：以下内容不会发送 —");
        ctx.events.info(format!("System:\n{}", prompts::PARSE_SYSTEM));
        ctx.events.info(format!("User:\n{user}"));
        return Ok(Breakdown::default());
    }

    let provider = ctx.factory().chat()?;
    ctx.events
        .info(format!("调用 {} 解析剧本…", provider.endpoint()));
    ctx.events.progress(0, 1, "等待模型返回");
    ctx.check_cancel()?;

    let value = provider
        .complete_json(ChatJsonRequest {
            system: prompts::PARSE_SYSTEM,
            user: &user,
            schema_name: "breakdown",
            schema: &schema,
        })
        .await?;

    let mut breakdown: Breakdown =
        serde_json::from_value(value).context("模型返回的 JSON 不符合 breakdown 结构")?;

    if breakdown.title.trim().is_empty() {
        breakdown.title = ctx.config().name.clone();
    }
    let max = ctx.config().generation.max_shot_seconds;
    for shot in &mut breakdown.shots {
        shot.duration_secs = shot.duration_secs.clamp(2, max);
    }

    for issue in breakdown.lint() {
        ctx.events.warn(issue);
    }

    ctx.project.save_breakdown(&breakdown)?;
    ctx.events.artifact(ctx.project.breakdown_path());
    ctx.events.progress(1, 1, "解析完成");
    ctx.events.info(format!(
        "角色 {} · 场景 {} · 场 {} · 镜头 {}（约 {} 秒）",
        breakdown.characters.len(),
        breakdown.locations.len(),
        breakdown.scenes.len(),
        breakdown.shots.len(),
        breakdown.total_seconds()
    ));

    Ok(breakdown)
}
