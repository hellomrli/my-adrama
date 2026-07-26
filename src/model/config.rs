//! Project configuration.
//!
//! The old layout was a flat struct with one set of `<vendor>_*` fields per
//! provider, which meant adding a provider touched a dozen places. Here a
//! project holds a *map* of provider settings plus a small routing table that
//! says which provider serves each capability. Legacy `project.toml` files are
//! migrated transparently on load (see [`RawConfig`]).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use anyhow::{bail, Result};

/// A backend vendor family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "google")]
    Google,
    #[serde(rename = "xai")]
    Xai,
}

impl ProviderId {
    pub const ALL: [ProviderId; 3] = [ProviderId::OpenAi, ProviderId::Google, ProviderId::Xai];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::OpenAi => "openai",
            ProviderId::Google => "google",
            ProviderId::Xai => "xai",
        }
    }

    /// Short vendor label used in both CLI output and the GUI.
    pub fn label(self) -> &'static str {
        match self {
            ProviderId::OpenAi => "OpenAI",
            ProviderId::Google => "Google",
            ProviderId::Xai => "xAI",
        }
    }

    pub fn tagline(self) -> &'static str {
        match self {
            ProviderId::OpenAi => "GPT 对话 · gpt-image 图像",
            ProviderId::Google => "Gemini 对话 · Imagen 图像 · Veo 视频",
            ProviderId::Xai => "Grok 对话 · Grok 图像",
        }
    }

    /// Capabilities this vendor can actually serve. Routing a capability to a
    /// provider that does not support it is rejected up front instead of
    /// silently calling the wrong HTTP shape.
    pub fn supports(self, cap: Capability) -> bool {
        matches!(
            (self, cap),
            (ProviderId::OpenAi, Capability::Chat | Capability::Image)
                | (ProviderId::Google, _)
                | (ProviderId::Xai, Capability::Chat | Capability::Image)
        )
    }

    pub fn official_base_url(self) -> &'static str {
        match self {
            ProviderId::OpenAi => "https://api.openai.com/v1",
            ProviderId::Google => "https://generativelanguage.googleapis.com/v1beta",
            ProviderId::Xai => "https://api.x.ai/v1",
        }
    }

    /// Environment variables consulted for the official key, in priority order.
    pub fn official_env_keys(self) -> &'static [&'static str] {
        match self {
            ProviderId::OpenAi => &["OPENAI_API_KEY", "ADRAMA_OPENAI_API_KEY"],
            ProviderId::Google => &["GEMINI_API_KEY", "GOOGLE_API_KEY", "ADRAMA_GEMINI_API_KEY"],
            ProviderId::Xai => &["XAI_API_KEY", "GROK_API_KEY", "ADRAMA_XAI_API_KEY"],
        }
    }

    /// Environment variables consulted for the custom-endpoint key.
    pub fn custom_env_keys(self) -> &'static [&'static str] {
        match self {
            ProviderId::OpenAi => &["ADRAMA_OPENAI_CUSTOM_KEY"],
            ProviderId::Google => &["ADRAMA_GOOGLE_CUSTOM_KEY"],
            ProviderId::Xai => &["ADRAMA_XAI_CUSTOM_KEY"],
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" | "open_ai" | "gpt" | "image2" => Ok(ProviderId::OpenAi),
            "google" | "gemini" | "veo" | "omni" => Ok(ProviderId::Google),
            "xai" | "x_ai" | "grok" => Ok(ProviderId::Xai),
            other => bail!("未知服务商：{other}（可选 openai / google / xai）"),
        }
    }
}

/// What a provider is being asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Chat,
    Image,
    Video,
}

impl Capability {
    pub const ALL: [Capability; 3] = [Capability::Chat, Capability::Image, Capability::Video];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Chat => "chat",
            Capability::Image => "image",
            Capability::Video => "video",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Capability::Chat => "对话",
            Capability::Image => "图像",
            Capability::Video => "视频",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Capability::Chat => "剧本解析（结构化 JSON）",
            Capability::Image => "资产图与分镜图",
            Capability::Video => "分镜图生视频",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Official cloud endpoint vs. a user-supplied proxy / self-hosted gateway.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    #[default]
    Official,
    Custom,
}

impl EndpointMode {
    pub const ALL: [EndpointMode; 2] = [EndpointMode::Official, EndpointMode::Custom];

    pub fn label(self) -> &'static str {
        match self {
            EndpointMode::Official => "官方",
            EndpointMode::Custom => "自定义",
        }
    }
}

/// Output frame shape. Kept as a closed set so provider-specific size strings
/// are derived in one place instead of being string-matched per call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AspectRatio {
    #[default]
    Landscape,
    Portrait,
    Square,
}

impl AspectRatio {
    pub const ALL: [AspectRatio; 3] = [
        AspectRatio::Landscape,
        AspectRatio::Portrait,
        AspectRatio::Square,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AspectRatio::Landscape => "16:9",
            AspectRatio::Portrait => "9:16",
            AspectRatio::Square => "1:1",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        match s.trim() {
            "9:16" | "portrait" | "vertical" => AspectRatio::Portrait,
            "1:1" | "square" => AspectRatio::Square,
            _ => AspectRatio::Landscape,
        }
    }

    /// `size` parameter for OpenAI-compatible image endpoints.
    pub fn openai_size(self) -> &'static str {
        match self {
            AspectRatio::Landscape => "1536x1024",
            AspectRatio::Portrait => "1024x1536",
            AspectRatio::Square => "1024x1024",
        }
    }

    /// Nominal pixel size, used for local previews and letterboxing hints.
    pub fn nominal_size(self) -> (u32, u32) {
        match self {
            AspectRatio::Landscape => (1536, 1024),
            AspectRatio::Portrait => (1024, 1536),
            AspectRatio::Square => (1024, 1024),
        }
    }
}

impl fmt::Display for AspectRatio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for AspectRatio {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AspectRatio {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(AspectRatio::parse_lossy(&s))
    }
}

/// Endpoint + model settings for a single vendor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSettings {
    #[serde(default)]
    pub mode: EndpointMode,
    /// Base URL used when `mode == Custom`.
    #[serde(default)]
    pub custom_base_url: String,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub image_model: String,
    #[serde(default)]
    pub video_model: String,
    /// Optional override for the video endpoint when a proxy splits traffic.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub video_base_url: String,
}

impl ProviderSettings {
    pub fn defaults_for(id: ProviderId) -> Self {
        match id {
            ProviderId::OpenAi => Self {
                mode: EndpointMode::Official,
                custom_base_url: "http://127.0.0.1:8080/v1".into(),
                chat_model: "gpt-4.1".into(),
                image_model: "gpt-image-1".into(),
                video_model: String::new(),
                video_base_url: String::new(),
            },
            ProviderId::Google => Self {
                mode: EndpointMode::Official,
                custom_base_url: "http://127.0.0.1:8081/v1beta".into(),
                chat_model: "gemini-2.0-flash".into(),
                image_model: "imagen-3.0-generate-002".into(),
                video_model: "veo-3.1-generate-preview".into(),
                video_base_url: String::new(),
            },
            ProviderId::Xai => Self {
                mode: EndpointMode::Official,
                custom_base_url: "http://127.0.0.1:8082/v1".into(),
                chat_model: "grok-2-latest".into(),
                image_model: "grok-2-image".into(),
                video_model: String::new(),
                video_base_url: String::new(),
            },
        }
    }

    pub fn model_for(&self, cap: Capability) -> &str {
        match cap {
            Capability::Chat => &self.chat_model,
            Capability::Image => &self.image_model,
            Capability::Video => &self.video_model,
        }
    }

    pub fn model_for_mut(&mut self, cap: Capability) -> &mut String {
        match cap {
            Capability::Chat => &mut self.chat_model,
            Capability::Image => &mut self.image_model,
            Capability::Video => &mut self.video_model,
        }
    }

    /// Effective base URL for a capability, honouring mode and per-capability
    /// overrides. Trailing slashes are stripped so callers can always `format!`.
    pub fn base_url(&self, id: ProviderId, cap: Capability) -> String {
        let base = match self.mode {
            EndpointMode::Official => id.official_base_url().to_string(),
            EndpointMode::Custom => {
                let custom = self.custom_base_url.trim();
                if custom.is_empty() {
                    id.official_base_url().to_string()
                } else {
                    custom.to_string()
                }
            }
        };
        if cap == Capability::Video && !self.video_base_url.trim().is_empty() {
            return self.video_base_url.trim().trim_end_matches('/').to_string();
        }
        base.trim_end_matches('/').to_string()
    }
}

/// Which provider serves each capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    pub chat: ProviderId,
    pub image: ProviderId,
    pub video: ProviderId,
}

impl Default for Routing {
    fn default() -> Self {
        Self {
            chat: ProviderId::OpenAi,
            image: ProviderId::OpenAi,
            video: ProviderId::Google,
        }
    }
}

impl Routing {
    pub fn get(&self, cap: Capability) -> ProviderId {
        match cap {
            Capability::Chat => self.chat,
            Capability::Image => self.image,
            Capability::Video => self.video,
        }
    }

    pub fn set(&mut self, cap: Capability, id: ProviderId) {
        match cap {
            Capability::Chat => self.chat = id,
            Capability::Image => self.image = id,
            Capability::Video => self.video = id,
        }
    }
}

/// Knobs that shape how aggressively the pipeline calls paid APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationSettings {
    /// Parallel image requests (assets + storyboard).
    pub image_concurrency: usize,
    /// Parallel video jobs. Video is expensive, so this defaults to 1.
    pub video_concurrency: usize,
    /// Hard cap applied to per-shot duration (Veo tops out at 8s).
    pub max_shot_seconds: u32,
    /// Seconds between long-running video operation polls.
    pub video_poll_secs: u64,
    /// Give up on a video operation after this many seconds.
    pub video_timeout_secs: u64,
    /// Attempts per HTTP call before surfacing the error.
    pub request_retries: u32,
}

impl Default for GenerationSettings {
    fn default() -> Self {
        Self {
            image_concurrency: 3,
            video_concurrency: 1,
            max_shot_seconds: 8,
            video_poll_secs: 10,
            video_timeout_secs: 30 * 60,
            request_retries: 3,
        }
    }
}

/// `project.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawConfig")]
pub struct ProjectConfig {
    pub name: String,
    /// Style prefix prepended to every image prompt.
    pub style: String,
    pub aspect: AspectRatio,
    #[serde(default)]
    pub routing: Routing,
    #[serde(default)]
    pub generation: GenerationSettings,
    /// Per-vendor endpoints and model ids.
    pub providers: BTreeMap<ProviderId, ProviderSettings>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "untitled".into(),
            style: "cinematic, photorealistic, film grain".into(),
            aspect: AspectRatio::Landscape,
            routing: Routing::default(),
            generation: GenerationSettings::default(),
            providers: ProviderId::ALL
                .into_iter()
                .map(|id| (id, ProviderSettings::defaults_for(id)))
                .collect(),
        }
    }
}

impl ProjectConfig {
    pub fn new(name: &str, style: &str, aspect: AspectRatio) -> Self {
        Self {
            name: name.into(),
            style: style.into(),
            aspect,
            ..Self::default()
        }
    }

    pub fn provider(&self, id: ProviderId) -> &ProviderSettings {
        self.providers
            .get(&id)
            .unwrap_or_else(|| panic!("provider {id} missing from config"))
    }

    pub fn provider_mut(&mut self, id: ProviderId) -> &mut ProviderSettings {
        self.providers
            .entry(id)
            .or_insert_with(|| ProviderSettings::defaults_for(id))
    }

    /// Provider assigned to `cap`, together with its resolved endpoint.
    pub fn endpoint(&self, cap: Capability) -> Endpoint {
        let id = self.routing.get(cap);
        let settings = self.provider(id);
        Endpoint {
            provider: id,
            capability: cap,
            mode: settings.mode,
            base_url: settings.base_url(id, cap),
            model: settings.model_for(cap).trim().to_string(),
        }
    }

    /// Fill in any provider entry a hand-edited file left out.
    pub fn normalize(&mut self) {
        for id in ProviderId::ALL {
            let defaults = ProviderSettings::defaults_for(id);
            let entry = self.providers.entry(id).or_insert_with(|| defaults.clone());
            if entry.chat_model.trim().is_empty() {
                entry.chat_model = defaults.chat_model;
            }
            if entry.image_model.trim().is_empty() {
                entry.image_model = defaults.image_model;
            }
            if entry.video_model.trim().is_empty() {
                entry.video_model = defaults.video_model;
            }
            if entry.custom_base_url.trim().is_empty() {
                entry.custom_base_url = defaults.custom_base_url;
            }
        }
        self.generation.image_concurrency = self.generation.image_concurrency.clamp(1, 16);
        self.generation.video_concurrency = self.generation.video_concurrency.clamp(1, 8);
        self.generation.max_shot_seconds = self.generation.max_shot_seconds.clamp(2, 60);
        self.generation.video_poll_secs = self.generation.video_poll_secs.clamp(2, 300);
        self.generation.request_retries = self.generation.request_retries.clamp(1, 10);
    }

    /// Routing entries pointing at a provider that cannot serve them.
    pub fn routing_conflicts(&self) -> Vec<(Capability, ProviderId)> {
        Capability::ALL
            .into_iter()
            .filter_map(|cap| {
                let id = self.routing.get(cap);
                (!id.supports(cap)).then_some((cap, id))
            })
            .collect()
    }
}

/// A fully resolved call target: who, where, which model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub provider: ProviderId,
    pub capability: Capability,
    pub mode: EndpointMode,
    pub base_url: String,
    pub model: String,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} · {} · {}",
            self.provider.label(),
            self.mode.label(),
            if self.model.is_empty() {
                "(未设置模型)"
            } else {
                &self.model
            }
        )
    }
}

// ---------------------------------------------------------------------------
// Legacy migration
// ---------------------------------------------------------------------------

/// Deserialization target accepting both the current nested layout and the
/// original flat `<vendor>_*` layout shipped in 0.1.x.
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    #[serde(default = "default_name")]
    name: String,
    #[serde(default = "default_style")]
    style: String,
    #[serde(default)]
    aspect: Option<AspectRatio>,

    #[serde(default)]
    routing: Option<Routing>,
    #[serde(default)]
    generation: Option<GenerationSettings>,
    #[serde(default)]
    providers: Option<BTreeMap<ProviderId, ProviderSettings>>,

    // --- legacy flat fields (0.1.x) ---
    #[serde(default)]
    openai_mode: Option<EndpointMode>,
    #[serde(default)]
    openai_chat_model: Option<String>,
    #[serde(default)]
    openai_image_model: Option<String>,
    #[serde(default)]
    openai_custom_base_url: Option<String>,
    #[serde(default)]
    google_mode: Option<EndpointMode>,
    #[serde(default)]
    google_chat_model: Option<String>,
    #[serde(default)]
    google_image_model: Option<String>,
    #[serde(default)]
    google_video_model: Option<String>,
    #[serde(default)]
    google_custom_base_url: Option<String>,
    #[serde(default)]
    xai_mode: Option<EndpointMode>,
    #[serde(default)]
    xai_chat_model: Option<String>,
    #[serde(default)]
    xai_image_model: Option<String>,
    #[serde(default)]
    xai_video_model: Option<String>,
    #[serde(default)]
    xai_custom_base_url: Option<String>,
    #[serde(default)]
    xai_video_base_url: Option<String>,
    #[serde(default)]
    chat_provider: Option<ProviderId>,
    #[serde(default)]
    image_provider: Option<ProviderId>,
    #[serde(default)]
    video_provider: Option<ProviderId>,
}

fn default_name() -> String {
    "untitled".into()
}

fn default_style() -> String {
    "cinematic, photorealistic, film grain".into()
}

impl From<RawConfig> for ProjectConfig {
    fn from(raw: RawConfig) -> Self {
        let mut cfg = ProjectConfig {
            name: raw.name,
            style: raw.style,
            aspect: raw.aspect.unwrap_or_default(),
            routing: raw.routing.unwrap_or_default(),
            generation: raw.generation.unwrap_or_default(),
            providers: raw.providers.unwrap_or_default(),
        };

        // Legacy routing keys win only when the nested table is absent.
        if raw.routing.is_none() {
            if let Some(p) = raw.chat_provider {
                cfg.routing.chat = p;
            }
            if let Some(p) = raw.image_provider {
                cfg.routing.image = p;
            }
            if let Some(p) = raw.video_provider {
                cfg.routing.video = p;
            }
        }

        let legacy = [
            (
                ProviderId::OpenAi,
                raw.openai_mode,
                raw.openai_chat_model,
                raw.openai_image_model,
                None,
                raw.openai_custom_base_url,
                None,
            ),
            (
                ProviderId::Google,
                raw.google_mode,
                raw.google_chat_model,
                raw.google_image_model,
                raw.google_video_model,
                raw.google_custom_base_url,
                None,
            ),
            (
                ProviderId::Xai,
                raw.xai_mode,
                raw.xai_chat_model,
                raw.xai_image_model,
                raw.xai_video_model,
                raw.xai_custom_base_url,
                raw.xai_video_base_url,
            ),
        ];

        for (id, mode, chat, image, video, custom_base, video_base) in legacy {
            let entry = cfg
                .providers
                .entry(id)
                .or_insert_with(|| ProviderSettings::defaults_for(id));
            if let Some(m) = mode {
                entry.mode = m;
            }
            if let Some(v) = chat.filter(|s| !s.trim().is_empty()) {
                entry.chat_model = v;
            }
            if let Some(v) = image.filter(|s| !s.trim().is_empty()) {
                entry.image_model = v;
            }
            if let Some(v) = video.filter(|s| !s.trim().is_empty()) {
                entry.video_model = v;
            }
            if let Some(v) = custom_base.filter(|s| !s.trim().is_empty()) {
                entry.custom_base_url = v;
            }
            if let Some(v) = video_base.filter(|s| !s.trim().is_empty()) {
                entry.video_base_url = v;
            }
        }

        cfg.normalize();
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_flat_config_migrates() {
        let toml_src = r#"
            name = "my-drama"
            style = "cinematic"
            aspect = "9:16"
            openai_mode = "custom"
            openai_chat_model = "gpt-4.1-mini"
            openai_custom_base_url = "https://proxy.example.com/v1"
            google_video_model = "veo-3.1-generate-preview"
            chat_provider = "openai"
            image_provider = "google"
            video_provider = "google"
        "#;

        let cfg: ProjectConfig = toml::from_str(toml_src).expect("legacy config parses");
        assert_eq!(cfg.aspect, AspectRatio::Portrait);
        assert_eq!(cfg.routing.image, ProviderId::Google);
        assert_eq!(cfg.routing.video, ProviderId::Google);

        let openai = cfg.provider(ProviderId::OpenAi);
        assert_eq!(openai.mode, EndpointMode::Custom);
        assert_eq!(openai.chat_model, "gpt-4.1-mini");
        // Untouched legacy fields fall back to defaults rather than empty strings.
        assert_eq!(openai.image_model, "gpt-image-1");

        let chat = cfg.endpoint(Capability::Chat);
        assert_eq!(chat.base_url, "https://proxy.example.com/v1");
        assert_eq!(chat.model, "gpt-4.1-mini");
    }

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = ProjectConfig::new("demo", "noir", AspectRatio::Portrait);
        cfg.routing.image = ProviderId::Google;
        cfg.provider_mut(ProviderId::Google).mode = EndpointMode::Custom;
        cfg.provider_mut(ProviderId::Google).custom_base_url = "http://localhost:9/v1beta".into();

        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: ProjectConfig = toml::from_str(&text).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn official_mode_ignores_custom_base_url() {
        let cfg = ProjectConfig::default();
        let ep = cfg.endpoint(Capability::Chat);
        assert_eq!(ep.base_url, "https://api.openai.com/v1");
        assert_eq!(ep.provider, ProviderId::OpenAi);
    }

    #[test]
    fn unsupported_routing_is_detected() {
        let mut cfg = ProjectConfig::default();
        cfg.routing.video = ProviderId::OpenAi;
        let conflicts = cfg.routing_conflicts();
        assert_eq!(conflicts, vec![(Capability::Video, ProviderId::OpenAi)]);
    }

    #[test]
    fn video_base_url_override_applies_only_to_video() {
        let mut cfg = ProjectConfig::default();
        cfg.routing.video = ProviderId::Xai;
        cfg.routing.chat = ProviderId::Xai;
        let xai = cfg.provider_mut(ProviderId::Xai);
        xai.mode = EndpointMode::Custom;
        xai.custom_base_url = "https://proxy/v1".into();
        xai.video_base_url = "https://video-proxy/v1".into();

        assert_eq!(cfg.endpoint(Capability::Video).base_url, "https://video-proxy/v1");
        assert_eq!(cfg.endpoint(Capability::Chat).base_url, "https://proxy/v1");
    }
}
