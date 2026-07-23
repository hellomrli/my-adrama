use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use tracing::info;

use crate::api::openai::OpenAiClient;
use crate::model::Breakdown;
use crate::pipeline;
use crate::project::{Project, Stage, StageStatus};

const SYSTEM_PROMPT: &str = r#"You are a professional film breakdown assistant for short-drama production.
Parse the screenplay into a structured JSON breakdown for AI image/video generation.

Rules:
- IDs must be slug-like ascii: lowercase, digits, underscore (e.g. char_li_ming, shot_s01_03).
- Each shot duration_secs must be between 2 and 8 (Veo limit).
- Shots must cover the whole story in cinematic order.
- character_ids / prop_ids / location_id on shots must reference ids you defined.
- Visual descriptions must be concrete enough for image generation (wardrobe, lighting, framing).
- Return ONLY valid JSON matching the schema. No markdown."#;

pub async fn run(proj: &mut Project, dry_run: bool) -> Result<()> {
    let script_path = proj.find_script()?;
    let script = fs::read_to_string(&script_path)
        .with_context(|| format!("read script {}", script_path.display()))?;

    let user = format!(
        "Project title: {}\nAspect ratio: {}\nStyle: {}\n\n--- SCREENPLAY ---\n{}\n--- END ---",
        proj.config.name, proj.config.aspect, proj.config.style, script
    );

    if dry_run {
        println!("=== DRY RUN: parse ===");
        println!("Script: {}", script_path.display());
        println!("System:\n{SYSTEM_PROMPT}");
        println!("User:\n{user}");
        return Ok(());
    }

    pipeline::mark(proj, Stage::Parse, StageStatus::InProgress)?;

    let ep = proj.resolve_chat()?;
    let client = OpenAiClient::new(
        ep.api_key,
        &ep.base_url,
        &ep.model,
        &proj.config.openai_image_model,
    )?;

    info!("calling LLM to parse screenplay…");
    let schema = breakdown_schema();
    let value = match client
        .chat_json(SYSTEM_PROMPT, &user, "breakdown", schema)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            info!("strict schema failed ({e}); falling back to json_object mode");
            let loose_user = format!(
                "{user}\n\nRespond with a JSON object with keys: \
                 title, summary, characters, costumes, props, locations, scenes, shots.\n\
                 characters: [{{id,name,appearance,costume,personality}}]\n\
                 costumes: [{{id,name,description,character_id}}]\n\
                 props: [{{id,name,description}}]\n\
                 locations: [{{id,name,description,time_of_day}}]\n\
                 scenes: [{{id,number,title,description,location_id,time_of_day}}]\n\
                 shots: [{{id,scene_id,number,framing,camera,visual,dialogue,sfx,duration_secs,character_ids,prop_ids,location_id}}]"
            );
            client.chat_json_loose(SYSTEM_PROMPT, &loose_user).await?
        }
    };

    let mut breakdown: Breakdown =
        serde_json::from_value(value).context("deserialize breakdown JSON")?;
    if breakdown.title.is_empty() {
        breakdown.title = proj.config.name.clone();
    }

    // Normalize shot durations
    for shot in &mut breakdown.shots {
        if shot.duration_secs == 0 {
            shot.duration_secs = 5;
        }
        shot.duration_secs = shot.duration_secs.min(8).max(2);
    }

    proj.save_breakdown(&breakdown)?;
    pipeline::mark(proj, Stage::Parse, StageStatus::Done)?;

    println!(
        "解析完成 -> {}\n  角色: {}\n  场景地: {}\n  场: {}\n  镜头: {}",
        proj.parsed_path().display(),
        breakdown.characters.len(),
        breakdown.locations.len(),
        breakdown.scenes.len(),
        breakdown.shots.len()
    );
    println!("请检查 parsed/breakdown.json，然后: adrama approve parse");
    Ok(())
}

fn breakdown_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "summary", "characters", "costumes", "props", "locations", "scenes", "shots"],
        "properties": {
            "title": { "type": "string" },
            "summary": { "type": "string" },
            "characters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "name", "appearance", "costume", "personality"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "appearance": { "type": "string" },
                        "costume": { "type": "string" },
                        "personality": { "type": "string" }
                    }
                }
            },
            "costumes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "name", "description", "character_id"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "character_id": { "type": ["string", "null"] }
                    }
                }
            },
            "props": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "name", "description"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" }
                    }
                }
            },
            "locations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "name", "description", "time_of_day"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "time_of_day": { "type": "string" }
                    }
                }
            },
            "scenes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "number", "title", "description", "location_id", "time_of_day"],
                    "properties": {
                        "id": { "type": "string" },
                        "number": { "type": "integer" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "location_id": { "type": ["string", "null"] },
                        "time_of_day": { "type": "string" }
                    }
                }
            },
            "shots": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "id", "scene_id", "number", "framing", "camera", "visual",
                        "dialogue", "sfx", "duration_secs", "character_ids", "prop_ids", "location_id"
                    ],
                    "properties": {
                        "id": { "type": "string" },
                        "scene_id": { "type": "string" },
                        "number": { "type": "integer" },
                        "framing": { "type": "string" },
                        "camera": { "type": "string" },
                        "visual": { "type": "string" },
                        "dialogue": { "type": "string" },
                        "sfx": { "type": "string" },
                        "duration_secs": { "type": "integer" },
                        "character_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "prop_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "location_id": { "type": ["string", "null"] }
                    }
                }
            }
        }
    })
}
