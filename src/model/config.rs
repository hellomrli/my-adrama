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
            ProviderId::Xai => "Grok 对话 · 图像 · 视频",
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
                | (ProviderId::Xai, _)
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

impl FromStr for Capability {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" | "text" | "对话" => Ok(Capability::Chat),
            "image" | "img" | "图像" => Ok(Capability::Image),
            "video" | "视频" => Ok(Capability::Video),
            other => bail!("未知能力：{other}（可选 chat / image / video）"),
        }
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

/// 一种能力的完整端点配置：谁来做、连哪里、用哪个模型。
///
/// 三种能力各存一份，互不影响——同一家服务商在对话和生图上用不同中转、
/// 不同额度、不同模型是常态，共享只会让人改了一处却影响到另一处。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointConfig {
    pub provider: ProviderId,
    #[serde(default)]
    pub mode: EndpointMode,
    /// `mode == Custom` 时使用的地址。
    #[serde(default)]
    pub custom_base_url: String,
    #[serde(default)]
    pub model: String,
}

impl EndpointConfig {
    pub fn defaults_for(cap: Capability) -> Self {
        match cap {
            Capability::Chat => Self {
                provider: ProviderId::OpenAi,
                mode: EndpointMode::Official,
                custom_base_url: String::new(),
                model: "gpt-4.1".into(),
            },
            Capability::Image => Self {
                provider: ProviderId::OpenAi,
                mode: EndpointMode::Official,
                custom_base_url: String::new(),
                model: "gpt-image-1".into(),
            },
            Capability::Video => Self {
                provider: ProviderId::Google,
                mode: EndpointMode::Official,
                custom_base_url: String::new(),
                model: "veo-3.1-generate-preview".into(),
            },
        }
    }

    /// 该服务商在这种能力上的默认模型，用于切换服务商时给个合理起点。
    pub fn default_model(provider: ProviderId, cap: Capability) -> &'static str {
        match (provider, cap) {
            (ProviderId::OpenAi, Capability::Chat) => "gpt-4.1",
            (ProviderId::OpenAi, Capability::Image) => "gpt-image-1",
            (ProviderId::Google, Capability::Chat) => "gemini-2.0-flash",
            (ProviderId::Google, Capability::Image) => "imagen-3.0-generate-002",
            (ProviderId::Google, Capability::Video) => "veo-3.1-generate-preview",
            (ProviderId::Xai, Capability::Chat) => "grok-2-latest",
            (ProviderId::Xai, Capability::Image) => "grok-2-image",
            (ProviderId::Xai, Capability::Video) => "grok-video",
            _ => "",
        }
    }

    /// 生效的 base URL；末尾斜杠已去掉，调用方可以直接 `format!`。
    pub fn base_url(&self) -> String {
        let base = match self.mode {
            EndpointMode::Official => self.provider.official_base_url().to_string(),
            EndpointMode::Custom => {
                let custom = self.custom_base_url.trim();
                if custom.is_empty() {
                    self.provider.official_base_url().to_string()
                } else {
                    custom.to_string()
                }
            }
        };
        base.trim_end_matches('/').to_string()
    }

    /// 换服务商：模型换成新家的默认值，自定义地址留给用户自己填。
    pub fn switch_provider(&mut self, provider: ProviderId, cap: Capability) {
        if self.provider == provider {
            return;
        }
        self.provider = provider;
        self.model = Self::default_model(provider, cap).to_string();
        self.custom_base_url.clear();
    }
}

/// 生成相关的开关。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationSettings {
    /// 并行的图像请求数（资产 + 分镜）。
    pub image_concurrency: usize,
    /// 并行的视频任务数。视频很贵，默认串行。
    pub video_concurrency: usize,
    /// 单镜头时长上限（Veo 最长 8 秒）。
    pub max_shot_seconds: u32,
    /// 轮询长任务的间隔秒数。
    pub video_poll_secs: u64,
    /// 视频任务多久算超时。
    pub video_timeout_secs: u64,
    /// 每个 HTTP 请求的尝试次数。
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

/// `project.toml`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawConfig")]
pub struct ProjectConfig {
    pub name: String,
    /// 加在每个图像 prompt 前面的风格前缀。
    pub style: String,
    pub aspect: AspectRatio,
    #[serde(default)]
    pub generation: GenerationSettings,
    /// 每种能力一份端点配置。
    pub endpoints: BTreeMap<Capability, EndpointConfig>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "untitled".into(),
            style: "cinematic, photorealistic, film grain".into(),
            aspect: AspectRatio::Landscape,
            generation: GenerationSettings::default(),
            endpoints: Capability::ALL
                .into_iter()
                .map(|cap| (cap, EndpointConfig::defaults_for(cap)))
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

    pub fn slot(&self, cap: Capability) -> &EndpointConfig {
        self.endpoints
            .get(&cap)
            .unwrap_or_else(|| panic!("capability {cap} missing from config"))
    }

    pub fn slot_mut(&mut self, cap: Capability) -> &mut EndpointConfig {
        self.endpoints
            .entry(cap)
            .or_insert_with(|| EndpointConfig::defaults_for(cap))
    }

    /// 解析出这次要调用的目标。
    pub fn endpoint(&self, cap: Capability) -> Endpoint {
        let slot = self.slot(cap);
        Endpoint {
            provider: slot.provider,
            capability: cap,
            mode: slot.mode,
            base_url: slot.base_url(),
            model: slot.model.trim().to_string(),
        }
    }

    /// 补齐手改配置时漏掉的项。
    pub fn normalize(&mut self) {
        for cap in Capability::ALL {
            let slot = self
                .endpoints
                .entry(cap)
                .or_insert_with(|| EndpointConfig::defaults_for(cap));
            if slot.model.trim().is_empty() {
                slot.model = EndpointConfig::default_model(slot.provider, cap).to_string();
            }
        }
        self.generation.image_concurrency = self.generation.image_concurrency.clamp(1, 16);
        self.generation.video_concurrency = self.generation.video_concurrency.clamp(1, 8);
        self.generation.max_shot_seconds = self.generation.max_shot_seconds.clamp(2, 60);
        self.generation.video_poll_secs = self.generation.video_poll_secs.clamp(2, 300);
        self.generation.request_retries = self.generation.request_retries.clamp(1, 10);
    }

    /// 指到了不提供该能力的服务商。
    pub fn routing_conflicts(&self) -> Vec<(Capability, ProviderId)> {
        Capability::ALL
            .into_iter()
            .filter_map(|cap| {
                let provider = self.slot(cap).provider;
                (!provider.supports(cap)).then_some((cap, provider))
            })
            .collect()
    }
}

/// 一次调用的完整目标：谁、哪里、哪个模型。
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
// 旧格式迁移
// ---------------------------------------------------------------------------

/// 反序列化目标，同时接受当前格式、0.2.x 的 routing+providers、以及
/// 0.1.x 的平铺 `<vendor>_*` 字段。
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    #[serde(default = "default_name")]
    name: String,
    #[serde(default = "default_style")]
    style: String,
    #[serde(default)]
    aspect: Option<AspectRatio>,
    #[serde(default)]
    generation: Option<GenerationSettings>,

    // --- 当前格式 ---
    #[serde(default)]
    endpoints: Option<BTreeMap<Capability, EndpointConfig>>,

    // --- 0.2.x：能力路由 + 按服务商分组 ---
    #[serde(default)]
    routing: Option<LegacyRouting>,
    #[serde(default)]
    providers: Option<BTreeMap<ProviderId, LegacyProvider>>,

    // --- 0.1.x：平铺字段 ---
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
    xai_custom_base_url: Option<String>,
    #[serde(default)]
    chat_provider: Option<ProviderId>,
    #[serde(default)]
    image_provider: Option<ProviderId>,
    #[serde(default)]
    video_provider: Option<ProviderId>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct LegacyRouting {
    #[serde(default = "openai")]
    chat: ProviderId,
    #[serde(default = "openai")]
    image: ProviderId,
    #[serde(default = "google")]
    video: ProviderId,
}

fn openai() -> ProviderId {
    ProviderId::OpenAi
}

fn google() -> ProviderId {
    ProviderId::Google
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LegacyProvider {
    #[serde(default)]
    mode: EndpointMode,
    #[serde(default)]
    custom_base_url: String,
    #[serde(default)]
    chat_model: String,
    #[serde(default)]
    image_model: String,
    #[serde(default)]
    video_model: String,
}

impl LegacyProvider {
    fn model_for(&self, cap: Capability) -> &str {
        match cap {
            Capability::Chat => &self.chat_model,
            Capability::Image => &self.image_model,
            Capability::Video => &self.video_model,
        }
    }
}

fn default_name() -> String {
    "untitled".into()
}

fn default_style() -> String {
    "cinematic, photorealistic, film grain".into()
}

impl From<RawConfig> for ProjectConfig {
    fn from(raw: RawConfig) -> Self {
        let legacy_flat = legacy_flat_providers(&raw);
        let mut cfg = ProjectConfig {
            name: raw.name,
            style: raw.style,
            aspect: raw.aspect.unwrap_or_default(),
            generation: raw.generation.unwrap_or_default(),
            endpoints: raw.endpoints.unwrap_or_default(),
        };

        if cfg.endpoints.is_empty() {
            // 0.2.x：能力 → 服务商，再从该服务商那组设置里取端点与模型。
            let routing = raw.routing;
            let legacy_providers = raw.providers.clone().unwrap_or_default();

            for cap in Capability::ALL {
                let provider = routing
                    .map(|r| match cap {
                        Capability::Chat => r.chat,
                        Capability::Image => r.image,
                        Capability::Video => r.video,
                    })
                    .or(match cap {
                        Capability::Chat => raw.chat_provider,
                        Capability::Image => raw.image_provider,
                        Capability::Video => raw.video_provider,
                    })
                    .unwrap_or(EndpointConfig::defaults_for(cap).provider);

                let source = legacy_providers
                    .get(&provider)
                    .cloned()
                    .or_else(|| legacy_flat.get(&provider).cloned());

                let mut slot = EndpointConfig {
                    provider,
                    ..EndpointConfig::defaults_for(cap)
                };
                if let Some(source) = source {
                    slot.mode = source.mode;
                    slot.custom_base_url = source.custom_base_url.clone();
                    if !source.model_for(cap).trim().is_empty() {
                        slot.model = source.model_for(cap).to_string();
                    } else {
                        slot.model = EndpointConfig::default_model(provider, cap).to_string();
                    }
                } else {
                    slot.model = EndpointConfig::default_model(provider, cap).to_string();
                }
                cfg.endpoints.insert(cap, slot);
            }
        }

        cfg.normalize();
        cfg
    }
}

/// 把 0.1.x 的平铺字段收成「按服务商分组」的形状，好和 0.2.x 走同一条路。
fn legacy_flat_providers(raw: &RawConfig) -> BTreeMap<ProviderId, LegacyProvider> {
    let mut out = BTreeMap::new();
    let entries = [
        (
            ProviderId::OpenAi,
            raw.openai_mode,
            raw.openai_chat_model.clone(),
            raw.openai_image_model.clone(),
            None,
            raw.openai_custom_base_url.clone(),
        ),
        (
            ProviderId::Google,
            raw.google_mode,
            raw.google_chat_model.clone(),
            raw.google_image_model.clone(),
            raw.google_video_model.clone(),
            raw.google_custom_base_url.clone(),
        ),
        (
            ProviderId::Xai,
            raw.xai_mode,
            raw.xai_chat_model.clone(),
            raw.xai_image_model.clone(),
            None,
            raw.xai_custom_base_url.clone(),
        ),
    ];
    for (id, mode, chat, image, video, base) in entries {
        if mode.is_none() && chat.is_none() && image.is_none() && video.is_none() && base.is_none() {
            continue;
        }
        out.insert(
            id,
            LegacyProvider {
                mode: mode.unwrap_or_default(),
                custom_base_url: base.unwrap_or_default(),
                chat_model: chat.unwrap_or_default(),
                image_model: image.unwrap_or_default(),
                video_model: video.unwrap_or_default(),
            },
        );
    }
    out
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

        let cfg: ProjectConfig = toml::from_str(toml_src).expect("0.1.x 配置应能读入");
        assert_eq!(cfg.aspect, AspectRatio::Portrait);

        let chat = cfg.endpoint(Capability::Chat);
        assert_eq!(chat.provider, ProviderId::OpenAi);
        assert_eq!(chat.base_url, "https://proxy.example.com/v1");
        assert_eq!(chat.model, "gpt-4.1-mini");

        let image = cfg.endpoint(Capability::Image);
        assert_eq!(image.provider, ProviderId::Google);
        assert_eq!(image.model, "imagen-3.0-generate-002");
        // 生图走 Google，不该继承 OpenAI 的自定义地址
        assert!(image.base_url.contains("googleapis.com"));
    }

    #[test]
    fn v0_2_routing_config_migrates() {
        let toml_src = r#"
            name = "d"
            style = "s"
            aspect = "16:9"

            [routing]
            chat = "openai"
            image = "openai"
            video = "google"

            [providers.openai]
            mode = "custom"
            custom_base_url = "https://relay.example.com/v1"
            chat_model = "gpt-4.1"
            image_model = "gpt-image-1"
        "#;

        let cfg: ProjectConfig = toml::from_str(toml_src).expect("0.2.x 配置应能读入");
        // 迁移后两种能力各存一份，之后再改互不影响
        assert_eq!(cfg.slot(Capability::Chat).custom_base_url, "https://relay.example.com/v1");
        assert_eq!(cfg.slot(Capability::Image).custom_base_url, "https://relay.example.com/v1");
        assert_eq!(cfg.endpoint(Capability::Video).provider, ProviderId::Google);
    }

    #[test]
    fn capabilities_are_independent() {
        let mut cfg = ProjectConfig::default();
        cfg.slot_mut(Capability::Chat).mode = EndpointMode::Custom;
        cfg.slot_mut(Capability::Chat).custom_base_url = "https://chat-relay/v1".into();
        cfg.slot_mut(Capability::Chat).model = "gpt-4.1-mini".into();

        // 改对话不应动到生图，即使两者是同一家服务商
        let image = cfg.endpoint(Capability::Image);
        assert_eq!(image.provider, ProviderId::OpenAi);
        assert_eq!(image.mode, EndpointMode::Official);
        assert_eq!(image.base_url, "https://api.openai.com/v1");
        assert_eq!(image.model, "gpt-image-1");

        assert_eq!(cfg.endpoint(Capability::Chat).base_url, "https://chat-relay/v1");
    }

    #[test]
    fn switching_provider_resets_model_and_url() {
        let mut cfg = ProjectConfig::default();
        let slot = cfg.slot_mut(Capability::Image);
        slot.mode = EndpointMode::Custom;
        slot.custom_base_url = "https://openai-relay/v1".into();

        slot.switch_provider(ProviderId::Google, Capability::Image);
        assert_eq!(slot.model, "imagen-3.0-generate-002");
        // 上一家的中转地址对新家没有意义
        assert!(slot.custom_base_url.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = ProjectConfig::new("demo", "noir", AspectRatio::Portrait);
        cfg.slot_mut(Capability::Image).provider = ProviderId::Google;
        cfg.slot_mut(Capability::Image).mode = EndpointMode::Custom;
        cfg.slot_mut(Capability::Image).custom_base_url = "http://localhost:9/v1beta".into();

        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: ProjectConfig = toml::from_str(&text).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn official_mode_ignores_custom_base_url() {
        let mut cfg = ProjectConfig::default();
        cfg.slot_mut(Capability::Chat).custom_base_url = "https://ignored/v1".into();
        let ep = cfg.endpoint(Capability::Chat);
        assert_eq!(ep.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn unsupported_routing_is_detected() {
        let mut cfg = ProjectConfig::default();
        cfg.slot_mut(Capability::Video).provider = ProviderId::OpenAi;
        assert_eq!(
            cfg.routing_conflicts(),
            vec![(Capability::Video, ProviderId::OpenAi)]
        );
    }

    #[test]
    fn capability_parses_from_cli() {
        assert_eq!("image".parse::<Capability>().unwrap(), Capability::Image);
        assert_eq!("视频".parse::<Capability>().unwrap(), Capability::Video);
        assert!("nope".parse::<Capability>().is_err());
    }
}
