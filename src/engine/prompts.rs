//! Every prompt the pipeline sends, assembled in one place so it can be read,
//! reviewed and unit-tested without a network call.

use serde_json::{json, Value};

use crate::model::{
    AspectRatio, Breakdown, Character, Costume, Location, Prop, ProjectConfig, Shot,
};

/// Angles generated for each character sheet. Identity consistency downstream
/// depends on having a canonical front view.
pub const CHARACTER_VIEWS: &[(&str, &str)] = &[
    ("front", "正面半身肖像，直视镜头，中性表情"),
    ("side", "侧面轮廓，面向左侧"),
    ("full", "全身站姿，正面，从头到脚完整入镜"),
];

pub const PARSE_SYSTEM: &str = r#"你是短剧制作的专业拆解助手，负责把剧本拆成可直接驱动 AI 图像/视频生成的结构化 JSON。

规则：
- 所有 id 使用 ascii slug：小写字母、数字、下划线（如 char_li_ming、shot_s01_03）。
- 每个镜头 duration_secs 取 2–8 秒（视频模型单次上限）。
- 镜头需按叙事顺序覆盖整个故事，不得遗漏关键情节。
- 镜头上的 character_ids / prop_ids / location_id 必须引用你定义过的 id。
- 画面描述要具体到可画：服装、光线、景别、动作、情绪。
- 只返回符合 schema 的 JSON，不要 markdown、不要解释。"#;

/// Ask the model for a breakdown of this screenplay.
pub fn parse_user_prompt(config: &ProjectConfig, script: &str) -> String {
    format!(
        "项目名称：{}\n画幅：{}\n视觉风格：{}\n\n--- 剧本开始 ---\n{}\n--- 剧本结束 ---",
        config.name,
        config.aspect,
        config.style,
        script.trim()
    )
}

/// Append `label + body` as one sentence, without doubling the punctuation the
/// screenplay text already ends with.
fn push_clause(out: &mut String, label: &str, body: &str) {
    let body = body
        .trim()
        .trim_end_matches(['。', '．', '.', '，', ',', '；', ';', '、']);
    if body.is_empty() {
        return;
    }
    out.push_str(label);
    out.push_str(body);
    out.push('。');
}

/// Base identity prompt for a character; view suffixes are appended per image.
pub fn character_prompt(style: &str, ch: &Character) -> String {
    let mut s = format!(
        "{style}，角色设定参考图，保持同一张脸与同一身份。姓名：{}。外貌：{}。",
        ch.name, ch.appearance
    );
    push_clause(&mut s, "服装：", &ch.costume);
    push_clause(&mut s, "气质：", &ch.personality);
    s
}

pub fn character_view_prompt(base: &str, view_description: &str) -> String {
    format!("{base} 视角：{view_description}。中性灰背景，无文字、无水印。")
}

pub fn costume_prompt(style: &str, c: &Costume) -> String {
    format!(
        "{style}，服装设定图，平铺于中性灰背景，展示面料质感与配饰细节。描述：{}",
        c.description
    )
}

pub fn prop_prompt(style: &str, p: &Prop) -> String {
    format!(
        "{style}，道具参考照，单体居中，中性背景，无人物。描述：{}",
        p.description
    )
}

pub fn location_prompt(style: &str, l: &Location) -> String {
    let time = if l.time_of_day.trim().is_empty() {
        "不限".to_string()
    } else {
        l.time_of_day.clone()
    };
    format!(
        "{style}，场景全景空镜，画面中没有人物，时间：{time}。描述：{}",
        l.description
    )
}

/// Storyboard frame. Character appearance is repeated verbatim into the prompt
/// even though reference images are attached — redundancy measurably helps
/// identity consistency.
pub fn storyboard_prompt(style: &str, bd: &Breakdown, shot: &Shot) -> String {
    let mut s = format!(
        "{style}，短片单帧画面，无文字、无水印、无字幕。景别：{}。机位/运镜：{}。画面内容：{}。",
        shot.framing, shot.camera, shot.visual
    );

    for cid in &shot.character_ids {
        if let Some(ch) = bd.character(cid) {
            s.push_str(&format!("角色 {}：{}", ch.name, ch.appearance));
            if !ch.costume.trim().is_empty() {
                s.push_str(&format!("，身着{}", ch.costume));
            }
            s.push('。');
        }
    }

    if let Some(loc) = bd.location_for_shot(shot) {
        push_clause(&mut s, "场景：", &loc.description);
        push_clause(&mut s, "时间：", &loc.time_of_day);
    }

    push_clause(&mut s, "此刻台词语境：", &shot.dialogue);
    s
}

/// Image-to-video prompt; the storyboard frame supplies composition, so this
/// describes motion and performance only.
pub fn video_prompt(shot: &Shot) -> String {
    let mut s = format!(
        "以参考帧为首帧的电影感镜头。运镜：{}。动作与表演：{}。",
        shot.camera, shot.visual
    );
    push_clause(&mut s, "台词（如有口型请对齐）：", &shot.dialogue);
    push_clause(&mut s, "环境音/音效：", &shot.sfx);
    s.push_str("保持人物外貌与参考帧一致，不要出现文字或字幕。");
    s
}

/// Hint appended when the routed image model ignores reference images.
pub fn no_reference_warning(model: &str) -> String {
    format!(
        "当前图像模型 {model} 不支持参考图输入，角色一致性只能依赖文字描述；\
         如需一致性请把「图像」能力路由到支持参考图的模型。"
    )
}

/// JSON Schema for the breakdown. Strict mode requires every property to be
/// listed in `required`, so optional fields are modelled as nullable instead.
pub fn breakdown_schema() -> Value {
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
                        "character_ids": { "type": "array", "items": { "type": "string" } },
                        "prop_ids": { "type": "array", "items": { "type": "string" } },
                        "location_id": { "type": ["string", "null"] }
                    }
                }
            }
        }
    })
}

/// Frame size hint shown in dry-run output.
pub fn size_hint(aspect: AspectRatio) -> String {
    let (w, h) = aspect.nominal_size();
    format!("{aspect}（约 {w}×{h}）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::breakdown::Scene;
    use crate::model::Shot;

    fn breakdown() -> Breakdown {
        Breakdown {
            characters: vec![Character {
                id: "char_a".into(),
                name: "阿明".into(),
                appearance: "二十岁男性，短发，左眉有疤".into(),
                costume: "洗旧的蓝色工装".into(),
                personality: "沉默寡言".into(),
            }],
            locations: vec![Location {
                id: "loc_1".into(),
                name: "旧仓库".into(),
                description: "堆满木箱的废弃仓库".into(),
                time_of_day: "夜".into(),
            }],
            scenes: vec![Scene {
                id: "sc1".into(),
                number: 1,
                title: "对峙".into(),
                description: String::new(),
                location_id: Some("loc_1".into()),
                time_of_day: "夜".into(),
            }],
            shots: vec![Shot {
                id: "shot_s01_01".into(),
                scene_id: "sc1".into(),
                number: 1,
                framing: "中景".into(),
                camera: "手持缓推".into(),
                visual: "阿明推开仓库大门".into(),
                dialogue: "有人吗".into(),
                sfx: "铁门吱呀".into(),
                duration_secs: 6,
                character_ids: vec!["char_a".into()],
                prop_ids: vec![],
                location_id: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn storyboard_prompt_repeats_identity_and_inherits_scene_location() {
        let bd = breakdown();
        let p = storyboard_prompt("电影感", &bd, &bd.shots[0]);
        assert!(p.contains("左眉有疤"), "外貌应冗余写入：{p}");
        assert!(p.contains("蓝色工装"));
        // The shot has no location_id of its own — it must inherit the scene's.
        assert!(p.contains("废弃仓库"), "应继承场的场景描述：{p}");
        assert!(p.contains("无文字"));
    }

    #[test]
    fn video_prompt_describes_motion_not_composition() {
        let bd = breakdown();
        let p = video_prompt(&bd.shots[0]);
        assert!(p.contains("手持缓推"));
        assert!(p.contains("铁门吱呀"));
        assert!(p.contains("首帧"));
    }

    #[test]
    fn clauses_do_not_double_up_punctuation() {
        let mut bd = breakdown();
        bd.shots[0].dialogue = "别走。".into();
        let p = storyboard_prompt("写实", &bd, &bd.shots[0]);
        assert!(p.contains("此刻台词语境：别走。"), "{p}");
        assert!(!p.contains("别走。。"), "{p}");

        let v = video_prompt(&bd.shots[0]);
        assert!(!v.contains("。。"), "{v}");
    }

    #[test]
    fn character_prompt_omits_empty_fields() {
        let ch = Character {
            id: "c".into(),
            name: "无名".into(),
            appearance: "戴口罩".into(),
            costume: String::new(),
            personality: String::new(),
        };
        let p = character_prompt("写实", &ch);
        assert!(!p.contains("服装："));
        assert!(!p.contains("气质："));
        let view = character_view_prompt(&p, CHARACTER_VIEWS[0].1);
        assert!(view.contains("正面半身肖像"));
    }

    #[test]
    fn schema_covers_every_breakdown_field() {
        let schema = breakdown_schema();
        let required = schema["required"].as_array().unwrap();
        for key in ["characters", "costumes", "props", "locations", "scenes", "shots"] {
            assert!(required.iter().any(|v| v == key), "schema 缺少 {key}");
        }
        let shot_props = schema["properties"]["shots"]["items"]["properties"]
            .as_object()
            .unwrap();
        assert!(shot_props.contains_key("duration_secs"));
        assert!(shot_props.contains_key("character_ids"));
    }
}
