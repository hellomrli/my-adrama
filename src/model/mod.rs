use serde::{Deserialize, Serialize};

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
    /// e.g. wide / medium / close-up
    pub framing: String,
    /// camera angle / movement
    pub camera: String,
    pub visual: String,
    #[serde(default)]
    pub dialogue: String,
    #[serde(default)]
    pub sfx: String,
    /// seconds, <= 8 for Veo
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoryboardMeta {
    pub shot_id: String,
    pub prompt: String,
    #[serde(default)]
    pub reference_assets: Vec<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub status: ItemStatus,
}

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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    #[default]
    Pending,
    Generating,
    Done,
    Failed,
    Approved,
}
