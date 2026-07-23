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

/// Which backend to use for a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    OpenAi,
    Google,
    Xai,
    Custom,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Google => "google",
            ProviderKind::Xai => "xai",
            ProviderKind::Custom => "custom",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI / Image2",
            ProviderKind::Google => "Google / Gemini / Veo",
            ProviderKind::Xai => "xAI / Grok",
            ProviderKind::Custom => "自定义",
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
            "custom" => Ok(ProviderKind::Custom),
            _ => bail!("unknown provider: {s}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub style: String,
    /// e.g. "16:9" or "9:16"
    pub aspect: String,

    // --- OpenAI / Image2 (chat + image, OpenAI-compatible) ---
    #[serde(default = "default_openai_image_model")]
    pub openai_image_model: String,
    #[serde(default = "default_openai_chat_model")]
    pub openai_chat_model: String,
    #[serde(default = "default_openai_base_url")]
    pub openai_base_url: String,

    // --- Google / Gemini / Veo (video; historically called "omni") ---
    #[serde(default = "default_google_video_model")]
    pub google_video_model: String,
    #[serde(default = "default_google_base_url")]
    pub google_base_url: String,

    // --- xAI / Grok (OpenAI-compatible image/chat; optional video base) ---
    #[serde(default = "default_xai_base_url")]
    pub xai_base_url: String,
    #[serde(default = "default_xai_chat_model")]
    pub xai_chat_model: String,
    #[serde(default = "default_xai_image_model")]
    pub xai_image_model: String,
    #[serde(default = "default_xai_video_model")]
    pub xai_video_model: String,
    /// Optional separate video base (if empty, reuse xai_base_url)
    #[serde(default)]
    pub xai_video_base_url: String,

    // --- Custom OpenAI-compatible endpoints ---
    #[serde(default = "default_custom_base_url")]
    pub custom_base_url: String,
    #[serde(default)]
    pub custom_chat_model: String,
    #[serde(default)]
    pub custom_image_model: String,
    #[serde(default)]
    pub custom_video_model: String,
    #[serde(default)]
    pub custom_video_base_url: String,

    // --- Capability → provider routing ---
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
fn default_google_video_model() -> String {
    "veo-3.1-generate-preview".into()
}
fn default_google_base_url() -> String {
    "https://generativelanguage.googleapis.com/v1beta".into()
}
fn default_xai_base_url() -> String {
    "https://api.x.ai/v1".into()
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
fn default_custom_base_url() -> String {
    "http://127.0.0.1:8080/v1".into()
}
fn default_video_provider() -> ProviderKind {
    ProviderKind::Google
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "untitled".into(),
            style: "cinematic, photorealistic, film grain".into(),
            aspect: "16:9".into(),
            openai_image_model: default_openai_image_model(),
            openai_chat_model: default_openai_chat_model(),
            openai_base_url: default_openai_base_url(),
            google_video_model: default_google_video_model(),
            google_base_url: default_google_base_url(),
            xai_base_url: default_xai_base_url(),
            xai_chat_model: default_xai_chat_model(),
            xai_image_model: default_xai_image_model(),
            xai_video_model: default_xai_video_model(),
            xai_video_base_url: String::new(),
            custom_base_url: default_custom_base_url(),
            custom_chat_model: String::new(),
            custom_image_model: String::new(),
            custom_video_model: String::new(),
            custom_video_base_url: String::new(),
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

    pub fn openai_key() -> Result<String> {
        first_nonempty_env(&["OPENAI_API_KEY", "ADRAMA_OPENAI_API_KEY"])
            .context("OPENAI_API_KEY not set（请在「设置」中填写 Image2 / OpenAI Key）")
    }

    pub fn gemini_key() -> Result<String> {
        first_nonempty_env(&["GEMINI_API_KEY", "GOOGLE_API_KEY", "ADRAMA_GEMINI_API_KEY"])
            .context("GEMINI_API_KEY not set（请在「设置」中填写 Google / Veo / Omni Key）")
    }

    pub fn xai_key() -> Result<String> {
        first_nonempty_env(&["XAI_API_KEY", "GROK_API_KEY", "ADRAMA_XAI_API_KEY"])
            .context("XAI_API_KEY not set（请在「设置」中填写 Grok / xAI Key）")
    }

    pub fn custom_key() -> Result<String> {
        first_nonempty_env(&["ADRAMA_CUSTOM_API_KEY", "CUSTOM_API_KEY"])
            .context("ADRAMA_CUSTOM_API_KEY not set（请在「设置」中填写自定义 Key）")
    }

    /// Chat / LLM endpoint according to `chat_provider`.
    pub fn resolve_chat(&self) -> Result<ResolvedEndpoint> {
        self.resolve_openai_style(self.config.chat_provider, EndpointRole::Chat)
    }

    /// Image generation endpoint according to `image_provider`.
    pub fn resolve_image(&self) -> Result<ResolvedEndpoint> {
        self.resolve_openai_style(self.config.image_provider, EndpointRole::Image)
    }

    /// Video generation endpoint according to `video_provider`.
    pub fn resolve_video(&self) -> Result<ResolvedEndpoint> {
        let p = self.config.video_provider;
        match p {
            ProviderKind::OpenAi => Ok(ResolvedEndpoint {
                provider: p,
                base_url: self.config.openai_base_url.clone(),
                model: self.config.openai_image_model.clone(),
                api_key: Self::openai_key()?,
            }),
            ProviderKind::Google => Ok(ResolvedEndpoint {
                provider: p,
                base_url: self.config.google_base_url.clone(),
                model: self.config.google_video_model.clone(),
                api_key: Self::gemini_key()?,
            }),
            ProviderKind::Xai => {
                let base = if self.config.xai_video_base_url.trim().is_empty() {
                    self.config.xai_base_url.clone()
                } else {
                    self.config.xai_video_base_url.clone()
                };
                Ok(ResolvedEndpoint {
                    provider: p,
                    base_url: base,
                    model: self.config.xai_video_model.clone(),
                    api_key: Self::xai_key()?,
                })
            }
            ProviderKind::Custom => {
                let base = if self.config.custom_video_base_url.trim().is_empty() {
                    self.config.custom_base_url.clone()
                } else {
                    self.config.custom_video_base_url.clone()
                };
                let model = if self.config.custom_video_model.trim().is_empty() {
                    self.config.google_video_model.clone()
                } else {
                    self.config.custom_video_model.clone()
                };
                Ok(ResolvedEndpoint {
                    provider: p,
                    base_url: base,
                    model,
                    api_key: Self::custom_key()?,
                })
            }
        }
    }

    fn resolve_openai_style(
        &self,
        provider: ProviderKind,
        role: EndpointRole,
    ) -> Result<ResolvedEndpoint> {
        match provider {
            ProviderKind::OpenAi | ProviderKind::Google => {
                // Google chat/image still via OpenAI-compatible if user points openai url;
                // default chat/image stay on OpenAI fields when provider is OpenAi.
                // If user selects Google for chat/image, reuse google base with gemini key
                // (many proxies expose OpenAI-compatible routes).
                if provider == ProviderKind::Google {
                    let model = match role {
                        EndpointRole::Chat => self.config.openai_chat_model.clone(),
                        EndpointRole::Image => self.config.openai_image_model.clone(),
                    };
                    Ok(ResolvedEndpoint {
                        provider,
                        base_url: self.config.google_base_url.clone(),
                        model,
                        api_key: Self::gemini_key()?,
                    })
                } else {
                    let model = match role {
                        EndpointRole::Chat => self.config.openai_chat_model.clone(),
                        EndpointRole::Image => self.config.openai_image_model.clone(),
                    };
                    Ok(ResolvedEndpoint {
                        provider: ProviderKind::OpenAi,
                        base_url: self.config.openai_base_url.clone(),
                        model,
                        api_key: Self::openai_key()?,
                    })
                }
            }
            ProviderKind::Xai => {
                let model = match role {
                    EndpointRole::Chat => self.config.xai_chat_model.clone(),
                    EndpointRole::Image => self.config.xai_image_model.clone(),
                };
                Ok(ResolvedEndpoint {
                    provider,
                    base_url: self.config.xai_base_url.clone(),
                    model,
                    api_key: Self::xai_key()?,
                })
            }
            ProviderKind::Custom => {
                let model = match role {
                    EndpointRole::Chat => {
                        if self.config.custom_chat_model.trim().is_empty() {
                            self.config.openai_chat_model.clone()
                        } else {
                            self.config.custom_chat_model.clone()
                        }
                    }
                    EndpointRole::Image => {
                        if self.config.custom_image_model.trim().is_empty() {
                            self.config.openai_image_model.clone()
                        } else {
                            self.config.custom_image_model.clone()
                        }
                    }
                };
                Ok(ResolvedEndpoint {
                    provider,
                    base_url: self.config.custom_base_url.clone(),
                    model,
                    api_key: Self::custom_key()?,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum EndpointRole {
    Chat,
    Image,
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
