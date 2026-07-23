use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::model::Breakdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Parse,
    Assets,
    Storyboard,
    Video,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Parse => "parse",
            Stage::Assets => "assets",
            Stage::Storyboard => "storyboard",
            Stage::Video => "video",
        }
    }

    pub fn all() -> &'static [Stage] {
        &[
            Stage::Parse,
            Stage::Assets,
            Stage::Storyboard,
            Stage::Video,
        ]
    }

    pub fn prev(self) -> Option<Stage> {
        match self {
            Stage::Parse => None,
            Stage::Assets => Some(Stage::Parse),
            Stage::Storyboard => Some(Stage::Assets),
            Stage::Video => Some(Stage::Storyboard),
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Stage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "parse" => Ok(Stage::Parse),
            "assets" => Ok(Stage::Assets),
            "storyboard" => Ok(Stage::Storyboard),
            "video" => Ok(Stage::Video),
            _ => bail!("unknown stage: {s}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    #[default]
    Pending,
    InProgress,
    Done,
    Approved,
}

/// Which backend family to use for a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    OpenAi,
    Google,
    Xai,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Google => "google",
            ProviderKind::Xai => "xai",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI / Image2",
            ProviderKind::Google => "Google / Veo",
            ProviderKind::Xai => "xAI / Grok",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "openai" | "image2" | "gpt" => Ok(ProviderKind::OpenAi),
            "google" | "gemini" | "veo" | "omni" => Ok(ProviderKind::Google),
            "xai" | "grok" => Ok(ProviderKind::Xai),
            // legacy "custom" maps to OpenAI family with custom mode
            "custom" => Ok(ProviderKind::OpenAi),
            _ => bail!("unknown provider: {s}"),
        }
    }
}

/// Official cloud API vs user-defined proxy / self-host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    #[default]
    Official,
    Custom,
}

impl EndpointMode {
    pub fn label_zh(self) -> &'static str {
        match self {
            EndpointMode::Official => "官方",
            EndpointMode::Custom => "自定义",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub style: String,
    /// e.g. "16:9" or "9:16"
    pub aspect: String,

    // --- OpenAI / Image2 ---
    #[serde(default)]
    pub openai_mode: EndpointMode,
    #[serde(default = "default_openai_image_model")]
    pub openai_image_model: String,
    #[serde(default = "default_openai_chat_model")]
    pub openai_chat_model: String,
    /// Used when openai_mode = Custom (OpenAI-compatible base)
    #[serde(default = "default_openai_custom_base")]
    pub openai_custom_base_url: String,
    /// Legacy field — kept for older project.toml; ignored when openai_mode present
    #[serde(default = "default_openai_base_url")]
    pub openai_base_url: String,

    // --- Google / Gemini / Veo ---
    #[serde(default)]
    pub google_mode: EndpointMode,
    #[serde(default = "default_google_video_model")]
    pub google_video_model: String,
    #[serde(default = "default_google_chat_model")]
    pub google_chat_model: String,
    #[serde(default = "default_google_image_model")]
    pub google_image_model: String,
    #[serde(default = "default_google_custom_base")]
    pub google_custom_base_url: String,
    #[serde(default = "default_google_base_url")]
    pub google_base_url: String,

    // --- xAI / Grok ---
    #[serde(default)]
    pub xai_mode: EndpointMode,
    #[serde(default = "default_xai_chat_model")]
    pub xai_chat_model: String,
    #[serde(default = "default_xai_image_model")]
    pub xai_image_model: String,
    #[serde(default = "default_xai_video_model")]
    pub xai_video_model: String,
    #[serde(default = "default_xai_custom_base")]
    pub xai_custom_base_url: String,
    #[serde(default = "default_xai_base_url")]
    pub xai_base_url: String,
    #[serde(default)]
    pub xai_video_base_url: String,

    // --- Capability → provider family ---
    #[serde(default)]
    pub chat_provider: ProviderKind,
    #[serde(default)]
    pub image_provider: ProviderKind,
    #[serde(default = "default_video_provider")]
    pub video_provider: ProviderKind,
}

fn default_openai_image_model() -> String {
    "gpt-image-1".into()
}
fn default_openai_chat_model() -> String {
    "gpt-4.1".into()
}
fn default_openai_base_url() -> String {
    "https://api.openai.com/v1".into()
}
fn default_openai_custom_base() -> String {
    "http://127.0.0.1:8080/v1".into()
}
fn default_google_video_model() -> String {
    "veo-3.1-generate-preview".into()
}
fn default_google_chat_model() -> String {
    "gemini-2.0-flash".into()
}
fn default_google_image_model() -> String {
    "imagen-3.0-generate-002".into()
}
fn default_google_base_url() -> String {
    "https://generativelanguage.googleapis.com/v1beta".into()
}
fn default_google_custom_base() -> String {
    "http://127.0.0.1:8081/v1beta".into()
}
fn default_xai_base_url() -> String {
    "https://api.x.ai/v1".into()
}
fn default_xai_custom_base() -> String {
    "http://127.0.0.1:8082/v1".into()
}
fn default_xai_chat_model() -> String {
    "grok-2-latest".into()
}
fn default_xai_image_model() -> String {
    "grok-2-image".into()
}
fn default_xai_video_model() -> String {
    "grok-video".into()
}
fn default_video_provider() -> ProviderKind {
    ProviderKind::Google
}

pub const OPENAI_OFFICIAL_BASE: &str = "https://api.openai.com/v1";
pub const GOOGLE_OFFICIAL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const XAI_OFFICIAL_BASE: &str = "https://api.x.ai/v1";

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "untitled".into(),
            style: "cinematic, photorealistic, film grain".into(),
            aspect: "16:9".into(),
            openai_mode: EndpointMode::Official,
            openai_image_model: default_openai_image_model(),
            openai_chat_model: default_openai_chat_model(),
            openai_custom_base_url: default_openai_custom_base(),
            openai_base_url: default_openai_base_url(),
            google_mode: EndpointMode::Official,
            google_video_model: default_google_video_model(),
            google_chat_model: default_google_chat_model(),
            google_image_model: default_google_image_model(),
            google_custom_base_url: default_google_custom_base(),
            google_base_url: default_google_base_url(),
            xai_mode: EndpointMode::Official,
            xai_chat_model: default_xai_chat_model(),
            xai_image_model: default_xai_image_model(),
            xai_video_model: default_xai_video_model(),
            xai_custom_base_url: default_xai_custom_base(),
            xai_base_url: default_xai_base_url(),
            xai_video_base_url: String::new(),
            chat_provider: ProviderKind::OpenAi,
            image_provider: ProviderKind::OpenAi,
            video_provider: ProviderKind::Google,
        }
    }
}

/// Resolved HTTP endpoint for a capability.
#[derive(Debug, Clone)]
pub struct ResolvedEndpoint {
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectState {
    pub parse: StageStatus,
    pub assets: StageStatus,
    pub storyboard: StageStatus,
    pub video: StageStatus,
}

impl ProjectState {
    pub fn get(&self, stage: Stage) -> StageStatus {
        match stage {
            Stage::Parse => self.parse,
            Stage::Assets => self.assets,
            Stage::Storyboard => self.storyboard,
            Stage::Video => self.video,
        }
    }

    pub fn set(&mut self, stage: Stage, status: StageStatus) {
        match stage {
            Stage::Parse => self.parse = status,
            Stage::Assets => self.assets = status,
            Stage::Storyboard => self.storyboard = status,
            Stage::Video => self.video = status,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub config: ProjectConfig,
    pub state: ProjectState,
}

impl Project {
    pub fn create(path: &Path, name: &str, style: &str, aspect: &str) -> Result<Self> {
        if path.exists() && path.read_dir()?.next().is_some() {
            bail!("directory {} is not empty", path.display());
        }
        fs::create_dir_all(path)?;
        for sub in [
            "script",
            "parsed",
            "assets/characters",
            "assets/costumes",
            "assets/props",
            "assets/locations",
            "storyboard",
            "video",
        ] {
            fs::create_dir_all(path.join(sub))?;
        }

        let config = ProjectConfig {
            name: name.into(),
            style: style.into(),
            aspect: aspect.into(),
            ..ProjectConfig::default()
        };
        let state = ProjectState::default();

        let proj = Self {
            root: path.to_path_buf(),
            config,
            state,
        };
        proj.save_config()?;
        proj.save_state()?;
        Ok(proj)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let root = fs::canonicalize(path)
            .with_context(|| format!("cannot open project at {}", path.display()))?;
        let config_path = root.join("project.toml");
        if !config_path.exists() {
            bail!(
                "not an adrama project (missing project.toml): {}",
                root.display()
            );
        }
        let config_str = fs::read_to_string(&config_path)?;
        let config: ProjectConfig = toml::from_str(&config_str)
            .with_context(|| format!("invalid project.toml at {}", config_path.display()))?;

        let state_path = root.join("state.json");
        let state = if state_path.exists() {
            let s = fs::read_to_string(&state_path)?;
            serde_json::from_str(&s)?
        } else {
            ProjectState::default()
        };

        Ok(Self {
            root,
            config,
            state,
        })
    }

    pub fn save_config(&self) -> Result<()> {
        let s = toml::to_string_pretty(&self.config)?;
        fs::write(self.root.join("project.toml"), s)?;
        Ok(())
    }

    pub fn save_state(&self) -> Result<()> {
        let s = serde_json::to_string_pretty(&self.state)?;
        fs::write(self.root.join("state.json"), s)?;
        Ok(())
    }

    pub fn script_dir(&self) -> PathBuf {
        self.root.join("script")
    }

    pub fn parsed_path(&self) -> PathBuf {
        self.root.join("parsed/breakdown.json")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn storyboard_dir(&self) -> PathBuf {
        self.root.join("storyboard")
    }

    pub fn video_dir(&self) -> PathBuf {
        self.root.join("video")
    }

    pub fn load_breakdown(&self) -> Result<Breakdown> {
        let path = self.parsed_path();
        if !path.exists() {
            bail!("breakdown.json not found; run `adrama parse` first");
        }
        let s = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn save_breakdown(&self, breakdown: &Breakdown) -> Result<()> {
        fs::create_dir_all(self.root.join("parsed"))?;
        let s = serde_json::to_string_pretty(breakdown)?;
        fs::write(self.parsed_path(), s)?;
        Ok(())
    }

    pub fn find_script(&self) -> Result<PathBuf> {
        let dir = self.script_dir();
        if !dir.exists() {
            bail!("script/ directory missing");
        }
        let mut candidates: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e, "md" | "txt" | "fountain"))
                    .unwrap_or(false)
            })
            .collect();
        candidates.sort();
        candidates
            .into_iter()
            .next()
            .with_context(|| format!("no script file found in {}", dir.display()))
    }

    /// Map aspect ratio to OpenAI image size string.
    pub fn image_size(&self) -> &'static str {
        match self.config.aspect.as_str() {
            "9:16" => "1024x1536",
            "1:1" => "1024x1024",
            _ => "1536x1024", // 16:9-ish
        }
    }

    pub fn openai_key(mode: EndpointMode) -> Result<String> {
        match mode {
            EndpointMode::Official => first_nonempty_env(&["OPENAI_API_KEY", "ADRAMA_OPENAI_API_KEY"])
                .context("未设置 OpenAI 官方 Key（设置 → OpenAI / Image2 → 官方密钥）"),
            EndpointMode::Custom => first_nonempty_env(&[
                "ADRAMA_OPENAI_CUSTOM_KEY",
                "OPENAI_API_KEY",
            ])
            .context("未设置 OpenAI 自定义 Key（设置 → OpenAI / Image2 → 自定义密钥）"),
        }
    }

    pub fn google_key(mode: EndpointMode) -> Result<String> {
        match mode {
            EndpointMode::Official => first_nonempty_env(&[
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "ADRAMA_GEMINI_API_KEY",
            ])
            .context("未设置 Google 官方 Key（设置 → Google / Veo → 官方密钥）"),
            EndpointMode::Custom => first_nonempty_env(&[
                "ADRAMA_GOOGLE_CUSTOM_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
            ])
            .context("未设置 Google 自定义 Key（设置 → Google / Veo → 自定义密钥）"),
        }
    }

    pub fn xai_key(mode: EndpointMode) -> Result<String> {
        match mode {
            EndpointMode::Official => {
                first_nonempty_env(&["XAI_API_KEY", "GROK_API_KEY", "ADRAMA_XAI_API_KEY"])
                    .context("未设置 Grok 官方 Key（设置 → xAI / Grok → 官方密钥）")
            }
            EndpointMode::Custom => {
                first_nonempty_env(&["ADRAMA_XAI_CUSTOM_KEY", "XAI_API_KEY", "GROK_API_KEY"])
                    .context("未设置 Grok 自定义 Key（设置 → xAI / Grok → 自定义密钥）")
            }
        }
    }

    fn openai_base(&self) -> String {
        match self.config.openai_mode {
            EndpointMode::Official => OPENAI_OFFICIAL_BASE.to_string(),
            EndpointMode::Custom => {
                let u = self.config.openai_custom_base_url.trim();
                if u.is_empty() {
                    self.config.openai_base_url.clone()
                } else {
                    u.to_string()
                }
            }
        }
    }

    fn google_base(&self) -> String {
        match self.config.google_mode {
            EndpointMode::Official => GOOGLE_OFFICIAL_BASE.to_string(),
            EndpointMode::Custom => {
                let u = self.config.google_custom_base_url.trim();
                if u.is_empty() {
                    self.config.google_base_url.clone()
                } else {
                    u.to_string()
                }
            }
        }
    }

    fn xai_base(&self) -> String {
        match self.config.xai_mode {
            EndpointMode::Official => XAI_OFFICIAL_BASE.to_string(),
            EndpointMode::Custom => {
                let u = self.config.xai_custom_base_url.trim();
                if u.is_empty() {
                    self.config.xai_base_url.clone()
                } else {
                    u.to_string()
                }
            }
        }
    }

    /// Chat / LLM endpoint according to `chat_provider` + that family's mode.
    pub fn resolve_chat(&self) -> Result<ResolvedEndpoint> {
        self.resolve_role(self.config.chat_provider, EndpointRole::Chat)
    }

    /// Image generation endpoint.
    pub fn resolve_image(&self) -> Result<ResolvedEndpoint> {
        self.resolve_role(self.config.image_provider, EndpointRole::Image)
    }

    /// Video generation endpoint.
    pub fn resolve_video(&self) -> Result<ResolvedEndpoint> {
        self.resolve_role(self.config.video_provider, EndpointRole::Video)
    }

    fn resolve_role(
        &self,
        provider: ProviderKind,
        role: EndpointRole,
    ) -> Result<ResolvedEndpoint> {
        match provider {
            ProviderKind::OpenAi => {
                let mode = self.config.openai_mode;
                let model = match role {
                    EndpointRole::Chat => self.config.openai_chat_model.clone(),
                    EndpointRole::Image | EndpointRole::Video => {
                        self.config.openai_image_model.clone()
                    }
                };
                Ok(ResolvedEndpoint {
                    provider,
                    base_url: self.openai_base(),
                    model,
                    api_key: Self::openai_key(mode)?,
                })
            }
            ProviderKind::Google => {
                let mode = self.config.google_mode;
                let model = match role {
                    EndpointRole::Chat => self.config.google_chat_model.clone(),
                    EndpointRole::Image => self.config.google_image_model.clone(),
                    EndpointRole::Video => self.config.google_video_model.clone(),
                };
                Ok(ResolvedEndpoint {
                    provider,
                    base_url: self.google_base(),
                    model,
                    api_key: Self::google_key(mode)?,
                })
            }
            ProviderKind::Xai => {
                let mode = self.config.xai_mode;
                let model = match role {
                    EndpointRole::Chat => self.config.xai_chat_model.clone(),
                    EndpointRole::Image => self.config.xai_image_model.clone(),
                    EndpointRole::Video => self.config.xai_video_model.clone(),
                };
                let base = if role == EndpointRole::Video
                    && !self.config.xai_video_base_url.trim().is_empty()
                    && mode == EndpointMode::Custom
                {
                    self.config.xai_video_base_url.clone()
                } else {
                    self.xai_base()
                };
                Ok(ResolvedEndpoint {
                    provider,
                    base_url: base,
                    model,
                    api_key: Self::xai_key(mode)?,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointRole {
    Chat,
    Image,
    Video,
}

fn first_nonempty_env(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}
