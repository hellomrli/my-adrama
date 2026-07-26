//! Structured screenplay breakdown — the contract between stage 1 and the rest
//! of the pipeline (`parsed/breakdown.json`).

use serde::{Deserialize, Serialize};

use super::state::ItemStatus;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Breakdown {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub characters: Vec<Character>,
    #[serde(default)]
    pub costumes: Vec<Costume>,
    #[serde(default)]
    pub props: Vec<Prop>,
    #[serde(default)]
    pub locations: Vec<Location>,
    #[serde(default)]
    pub scenes: Vec<Scene>,
    #[serde(default)]
    pub shots: Vec<Shot>,
}

impl Breakdown {
    pub fn character(&self, id: &str) -> Option<&Character> {
        self.characters.iter().find(|c| c.id == id)
    }

    pub fn scene(&self, id: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id == id)
    }

    /// Location for a shot, falling back to the location of its scene.
    pub fn location_for_shot(&self, shot: &Shot) -> Option<&Location> {
        let id = shot
            .location_id
            .clone()
            .or_else(|| self.scene(&shot.scene_id).and_then(|s| s.location_id.clone()))?;
        self.locations.iter().find(|l| l.id == id)
    }

    /// Shots belonging to a 1-based scene number.
    pub fn shots_in_scene(&self, scene_number: u32) -> Vec<&Shot> {
        self.shots
            .iter()
            .filter(|shot| {
                self.scene(&shot.scene_id)
                    .map(|s| s.number == scene_number)
                    .unwrap_or(false)
            })
            .collect()
    }

    pub fn total_seconds(&self) -> u32 {
        self.shots.iter().map(|s| s.duration_secs).sum()
    }

    /// Referential integrity problems worth surfacing before spending money.
    pub fn lint(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.shots.is_empty() {
            issues.push("镜头表为空，后续阶段无事可做".into());
        }
        for shot in &self.shots {
            if self.scene(&shot.scene_id).is_none() {
                issues.push(format!("镜头 {} 引用了不存在的场 {}", shot.id, shot.scene_id));
            }
            for cid in &shot.character_ids {
                if self.character(cid).is_none() {
                    issues.push(format!("镜头 {} 引用了不存在的角色 {cid}", shot.id));
                }
            }
            if shot.visual.trim().is_empty() {
                issues.push(format!("镜头 {} 缺少画面描述", shot.id));
            }
        }
        for ch in &self.characters {
            if ch.appearance.trim().is_empty() {
                issues.push(format!("角色 {} 缺少外貌描述，一致性会很差", ch.name));
            }
        }
        issues
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub appearance: String,
    #[serde(default)]
    pub costume: String,
    #[serde(default)]
    pub personality: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Costume {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub character_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prop {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub time_of_day: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub number: u32,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub location_id: Option<String>,
    #[serde(default)]
    pub time_of_day: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    pub id: String,
    pub scene_id: String,
    pub number: u32,
    /// wide / medium / close-up …
    pub framing: String,
    /// camera angle and movement
    pub camera: String,
    pub visual: String,
    /// 镜头结束瞬间的画面（末帧依据；旧 breakdown 没有此字段则回退到 visual）。
    #[serde(default)]
    pub visual_end: String,
    #[serde(default)]
    pub dialogue: String,
    #[serde(default)]
    pub sfx: String,
    #[serde(default = "default_duration")]
    pub duration_secs: u32,
    #[serde(default)]
    pub character_ids: Vec<String>,
    #[serde(default)]
    pub prop_ids: Vec<String>,
    #[serde(default)]
    pub location_id: Option<String>,
}

fn default_duration() -> u32 {
    5
}

/// Sidecar written next to every generated asset (`meta.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssetMeta {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub status: ItemStatus,
    #[serde(default)]
    pub error: Option<String>,
    /// 用户自己放进来的素材：批量重生成时不会被覆盖。
    #[serde(default)]
    pub manual: bool,
}

/// Sidecar written next to every storyboard frame (`<shot>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoryboardMeta {
    pub shot_id: String,
    pub prompt: String,
    #[serde(default)]
    pub reference_assets: Vec<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// 末帧文件名。
    #[serde(default)]
    pub last_image: Option<String>,
    /// 本镜头的分镜帧数覆盖（None = 跟随全局设置）。
    #[serde(default)]
    pub frames: Option<u32>,
    #[serde(default)]
    pub status: ItemStatus,
    #[serde(default)]
    pub error: Option<String>,
    /// 用户自己放进来的画面：批量重生成时不会被覆盖。
    #[serde(default)]
    pub manual: bool,
}

/// 配音 sidecar（`voice/<shot>.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VoiceMeta {
    pub shot_id: String,
    /// 实际合成的台词文本。
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub voice: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub status: ItemStatus,
    #[serde(default)]
    pub error: Option<String>,
    /// 用户自己上传的配音：批量生成时不会被覆盖。
    #[serde(default)]
    pub manual: bool,
}

/// Sidecar written next to every clip (`<shot>.json`), including the
/// long-running operation id so an interrupted run can resume polling.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoMeta {
    pub shot_id: String,
    pub prompt: String,
    #[serde(default)]
    pub source_image: Option<String>,
    #[serde(default)]
    pub operation_name: Option<String>,
    #[serde(default)]
    pub video: Option<String>,
    #[serde(default)]
    pub status: ItemStatus,
    #[serde(default)]
    pub error: Option<String>,
    /// 用户自己放进来的片段：批量重生成时不会被覆盖。
    #[serde(default)]
    pub manual: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(id: &str, scene: &str) -> Shot {
        Shot {
            id: id.into(),
            scene_id: scene.into(),
            number: 1,
            framing: "medium".into(),
            camera: "static".into(),
            visual: "someone walks in".into(),
            visual_end: String::new(),
            dialogue: String::new(),
            sfx: String::new(),
            duration_secs: 5,
            character_ids: vec![],
            prop_ids: vec![],
            location_id: None,
        }
    }

    #[test]
    fn lint_flags_dangling_references() {
        let bd = Breakdown {
            shots: vec![shot("shot_1", "scene_missing")],
            ..Default::default()
        };
        let issues = bd.lint();
        assert!(issues.iter().any(|i| i.contains("scene_missing")));
    }

    #[test]
    fn shots_in_scene_filters_by_number() {
        let bd = Breakdown {
            scenes: vec![Scene {
                id: "sc1".into(),
                number: 2,
                title: "t".into(),
                description: String::new(),
                location_id: None,
                time_of_day: String::new(),
            }],
            shots: vec![shot("a", "sc1"), shot("b", "other")],
            ..Default::default()
        };
        let found = bd.shots_in_scene(2);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "a");
    }
}
