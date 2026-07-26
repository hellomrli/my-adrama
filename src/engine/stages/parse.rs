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
        // 完整 prompt 进日志文件；控制台只放摘要，否则整个剧本会把它糊满。
        tracing::info!("演练 · System:\n{}", prompts::PARSE_SYSTEM);
        tracing::info!("演练 · User:\n{user}");

        ctx.events
            .info("— 演练：以下内容只是预览，不会发送，也不会产生任何费用 —");
        ctx.events.info(format!(
            "将发往 {} 的提示词共 {} 字，开头是：\n{}",
            ctx.config().endpoint(crate::model::Capability::Chat),
            user.chars().count(),
            crate::model::index::truncate(&user, 400)
        ));
        ctx.events
            .info("（完整内容见日志文件：设置 → 关于与更新 → 打开日志文件）");
        ctx.events
            .warn("这只是演练。要真正调用模型，请关闭顶栏「演练模式」后再点「运行拆解」。");
        return Ok(Breakdown::default());
    }

    let provider = ctx.factory().chat()?;
    ctx.events
        .info(format!("调用 {} 解析剧本…", provider.endpoint()));
    ctx.events
        .progress(0, 0, "已发出请求，等待模型开始输出…");
    ctx.check_cancel()?;

    // 把流式接收情况实时报出去：否则长剧本 + 慢模型看起来就像卡死了。
    let events = ctx.events.clone();
    let last_log = std::sync::Mutex::new(std::time::Instant::now());
    let on_progress = move |p: crate::providers::http::SseProgress| {
        let detail = if p.chars > 0 {
            format!(
                "接收中 {} 字 · 已用 {} 秒",
                p.chars,
                p.elapsed.as_secs()
            )
        } else if p.thinking > 0 {
            format!("模型思考中 {} 段 · 已用 {} 秒", p.thinking, p.elapsed.as_secs())
        } else {
            format!("已连接 {} 段 · 已用 {} 秒", p.events, p.elapsed.as_secs())
        };
        events.progress(0, 0, detail);

        // 控制台里每 5 秒留一条，跑长任务时能看出是在动还是真卡住了。
        if let Ok(mut last) = last_log.lock() {
            if last.elapsed() >= std::time::Duration::from_secs(5) {
                *last = std::time::Instant::now();
                events.info(format!(
                    "… 接收中：{} 字 / {} 段（{} 秒）",
                    p.chars,
                    p.events,
                    p.elapsed.as_secs()
                ));
            }
        }
    };

    let value = provider
        .complete_json(ChatJsonRequest {
            system: prompts::PARSE_SYSTEM,
            user: &user,
            schema_name: "breakdown",
            schema: &schema,
            on_progress: Some(&on_progress),
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
