//! Provider abstraction.
//!
//! Stages talk to capabilities (`chat`, `image`, `video`), never to a vendor.
//! [`ProviderFactory`] turns the project's routing table plus a credential set
//! into the concrete client for a capability — and refuses up front if the
//! routed vendor cannot serve it, instead of sending an OpenAI-shaped request
//! to a Gemini endpoint.

pub mod google;
pub mod http;
pub mod openai;

use anyhow::{bail, Context, Result};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::model::{
    AspectRatio, Capability, Endpoint, EndpointMode, ProjectConfig, ProviderId,
};

// ---------------------------------------------------------------------------
// Capability traits
// ---------------------------------------------------------------------------

pub struct ChatJsonRequest<'a> {
    pub system: &'a str,
    pub user: &'a str,
    pub schema_name: &'a str,
    pub schema: &'a Value,
}

/// Structured-output text model.
pub trait ChatProvider: Send + Sync {
    fn endpoint(&self) -> &Endpoint;
    /// Return a JSON value conforming to `req.schema`.
    fn complete_json<'a>(&'a self, req: ChatJsonRequest<'a>) -> BoxFuture<'a, Result<Value>>;
}

pub struct ImageRequest<'a> {
    pub prompt: &'a str,
    pub aspect: AspectRatio,
    /// Reference images for identity consistency; may be empty.
    pub references: &'a [PathBuf],
}

/// Image generation / reference-guided editing. Returns encoded image bytes;
/// the caller decides where they land on disk.
pub trait ImageProvider: Send + Sync {
    fn endpoint(&self) -> &Endpoint;
    /// Whether reference images actually influence the result. When false the
    /// stage warns instead of silently losing character consistency.
    fn supports_references(&self) -> bool;
    fn generate<'a>(&'a self, req: ImageRequest<'a>) -> BoxFuture<'a, Result<Vec<u8>>>;
}

pub struct VideoRequest<'a> {
    pub prompt: &'a str,
    /// First frame.
    pub image: &'a Path,
    pub aspect: AspectRatio,
    pub duration_secs: u32,
}

/// A long-running video job that has not finished yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoPoll {
    Pending,
    Ready(Vec<u8>),
}

/// Image-to-video. Submission and polling are separate so the engine owns the
/// wait loop — that is where cancellation and progress reporting belong.
pub trait VideoProvider: Send + Sync {
    fn endpoint(&self) -> &Endpoint;
    /// Returns an operation id that survives a process restart.
    fn submit<'a>(&'a self, req: VideoRequest<'a>) -> BoxFuture<'a, Result<String>>;
    fn poll<'a>(&'a self, operation: &'a str) -> BoxFuture<'a, Result<VideoPoll>>;
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// API keys, held explicitly and passed into jobs.
///
/// The previous design pushed keys into process environment variables from the
/// UI thread while worker threads read them — a data race, and impossible to
/// scope per job. Keys now travel as values.
#[derive(Clone, Default)]
pub struct Credentials {
    /// 按「能力 + 服务商 + 端点模式」存放：对话和生图即使是同一家，
    /// 也可能用不同中转、不同额度的密钥。
    entries: BTreeMap<(Capability, ProviderId, EndpointMode), String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("configured", &self.entries.len())
            .finish()
    }
}

impl Credentials {
    pub fn set(
        &mut self,
        cap: Capability,
        id: ProviderId,
        mode: EndpointMode,
        key: impl Into<String>,
    ) {
        let key = key.into().trim().to_string();
        if key.is_empty() {
            self.entries.remove(&(cap, id, mode));
        } else {
            self.entries.insert((cap, id, mode), key);
        }
    }

    pub fn get(&self, cap: Capability, id: ProviderId, mode: EndpointMode) -> Option<&str> {
        self.entries.get(&(cap, id, mode)).map(|s| s.as_str())
    }

    pub fn has(&self, cap: Capability, id: ProviderId, mode: EndpointMode) -> bool {
        self.get(cap, id, mode).is_some()
    }

    /// 环境变量是按服务商给的，对三种能力都适用。
    pub fn from_env() -> Self {
        let mut creds = Self::default();
        for id in ProviderId::ALL {
            let official = first_env(id.official_env_keys());
            let custom = first_env(id.custom_env_keys());
            for cap in Capability::ALL {
                if let Some(k) = &official {
                    creds.set(cap, id, EndpointMode::Official, k.clone());
                }
                if let Some(k) = &custom {
                    creds.set(cap, id, EndpointMode::Custom, k.clone());
                }
            }
        }
        creds
    }

    /// Environment values fill gaps only; explicit settings win.
    pub fn fill_from_env(&mut self) {
        let env = Self::from_env();
        for (slot, key) in env.entries {
            self.entries.entry(slot).or_insert(key);
        }
    }

    pub fn require(&self, cap: Capability, id: ProviderId, mode: EndpointMode) -> Result<&str> {
        self.get(cap, id, mode).with_context(|| {
            format!(
                "「{}」还没有配置 {} 的{}密钥（设置 → 模型与密钥 → {}）",
                cap.label(),
                id.label(),
                mode.label(),
                cap.label()
            )
        })
    }
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Builds capability clients on demand, so a parse-only run never needs a video
/// key.
pub struct ProviderFactory<'a> {
    config: &'a ProjectConfig,
    credentials: &'a Credentials,
}

impl<'a> ProviderFactory<'a> {
    pub fn new(config: &'a ProjectConfig, credentials: &'a Credentials) -> Self {
        Self {
            config,
            credentials,
        }
    }

    fn resolve(&self, cap: Capability) -> Result<(Endpoint, String)> {
        let endpoint = self.config.endpoint(cap);
        if !endpoint.provider.supports(cap) {
            let alternatives: Vec<&str> = ProviderId::ALL
                .into_iter()
                .filter(|p| p.supports(cap))
                .map(|p| p.label())
                .collect();
            bail!(
                "{} 不提供{}能力（设置 → 能力路由）。可选：{}",
                endpoint.provider.label(),
                cap.label(),
                alternatives.join(" / ")
            );
        }
        if endpoint.model.is_empty() {
            bail!(
                "{} 的{}模型未填写（设置 → 服务商 → 模型 ID）",
                endpoint.provider.label(),
                cap.label()
            );
        }
        let key = self
            .credentials
            .require(cap, endpoint.provider, endpoint.mode)?
            .to_string();
        Ok((endpoint, key))
    }

    fn http(&self, key: &str, timeout: Duration) -> Result<http::Http> {
        http::Http::new(key, self.config.generation.request_retries, timeout)
    }

    pub fn chat(&self) -> Result<Arc<dyn ChatProvider>> {
        let (endpoint, key) = self.resolve(Capability::Chat)?;
        let http = self.http(&key, Duration::from_secs(300))?;
        Ok(match endpoint.provider {
            ProviderId::Google => Arc::new(google::GoogleClient::new(http, key, endpoint)),
            _ => Arc::new(openai::OpenAiCompatible::new(http, key, endpoint)),
        })
    }

    pub fn image(&self) -> Result<Arc<dyn ImageProvider>> {
        let (endpoint, key) = self.resolve(Capability::Image)?;
        let http = self.http(&key, Duration::from_secs(300))?;
        Ok(match endpoint.provider {
            ProviderId::Google => Arc::new(google::GoogleClient::new(http, key, endpoint)),
            _ => Arc::new(openai::OpenAiCompatible::new(http, key, endpoint)),
        })
    }

    pub fn video(&self) -> Result<Arc<dyn VideoProvider>> {
        let (endpoint, key) = self.resolve(Capability::Video)?;
        let http = self.http(&key, Duration::from_secs(180))?;
        match endpoint.provider {
            ProviderId::Google => Ok(Arc::new(google::GoogleClient::new(http, key, endpoint))),
            other => bail!("{} 暂不支持视频生成", other.label()),
        }
    }
}

// ---------------------------------------------------------------------------
// Connectivity probe (settings screen)
// ---------------------------------------------------------------------------

/// Result of a "test this key" click.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub summary: String,
    pub detail: String,
    /// Model ids the endpoint reports. Empty when a proxy does not implement
    /// `/models` — the UI then falls back to free-text entry.
    pub models: Vec<String>,
}

/// Heuristic: does this model id look like it serves `cap`? Used only to sort
/// the picker, never to hide a model — vendors rename things constantly.
pub fn looks_like(cap: Capability, model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    let image = ["image", "imagen", "dall-e", "dalle", "flux", "sd-", "stable"];
    let video = ["veo", "video", "sora", "kling", "runway"];
    match cap {
        Capability::Image => image.iter().any(|k| m.contains(k)),
        Capability::Video => video.iter().any(|k| m.contains(k)),
        Capability::Chat => {
            !image.iter().any(|k| m.contains(k))
                && !video.iter().any(|k| m.contains(k))
                && !m.contains("embedding")
                && !m.contains("whisper")
                && !m.contains("tts")
                && !m.contains("moderation")
                && !m.contains("rerank")
        }
    }
}

/// Verify a key/endpoint pair without spending generation credits.
pub async fn probe(
    id: ProviderId,
    mode: EndpointMode,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<ProbeReport> {
    let base = base_url.trim().trim_end_matches('/');
    let key = api_key.trim();
    if base.is_empty() {
        bail!("Base URL 为空");
    }
    if key.is_empty() {
        bail!("API Key 为空");
    }

    let http = http::Http::new(key, 1, Duration::from_secs(25))?;
    let label = format!("{} · {}", id.label(), mode.label());

    let models = match id {
        ProviderId::Google => google::list_models(&http, base, key).await,
        _ => openai::list_models(&http, base, key).await,
    }?;

    let mut detail = format!("端点 {base}\n密钥 {}", http::mask(key));
    if models.is_empty() {
        detail.push_str("\n该端点未返回模型列表（代理可能未实现 /models），模型 ID 需手动填写");
    } else {
        detail.push_str(&format!("\n拉取到 {} 个模型，可在下方下拉框中选择", models.len()));
        if !model.trim().is_empty() {
            let known = models.iter().any(|m| m == model.trim());
            detail.push_str(&format!(
                "\n当前配置 {model} {}",
                if known {
                    "✓ 在列表中"
                } else {
                    "· 不在列表中（可能已下线或代理未列出）"
                }
            ));
        }
    }

    Ok(ProbeReport {
        summary: if models.is_empty() {
            format!("{label} 连接正常")
        } else {
            format!("{label} 连接正常 · {} 个模型", models.len())
        },
        detail,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Capability;

    fn creds_with_all() -> Credentials {
        let mut c = Credentials::default();
        for cap in Capability::ALL {
            for id in ProviderId::ALL {
                c.set(cap, id, EndpointMode::Official, format!("key-{cap}-{id}"));
            }
        }
        c
    }

    #[test]
    fn unsupported_capability_is_rejected_before_any_request() {
        let mut config = ProjectConfig::default();
        config.slot_mut(Capability::Video).provider = ProviderId::Xai;
        let creds = creds_with_all();
        let err = ProviderFactory::new(&config, &creds)
            .video()
            .err()
            .expect("xAI has no video capability");
        assert!(err.to_string().contains("视频"), "{err}");
    }

    #[test]
    fn missing_key_names_the_capability_and_mode() {
        let config = ProjectConfig::default();
        let creds = Credentials::default();
        let err = ProviderFactory::new(&config, &creds)
            .chat()
            .err()
            .expect("no key configured");
        let msg = err.to_string();
        assert!(msg.contains("对话"), "{msg}");
        assert!(msg.contains("OpenAI"), "{msg}");
        assert!(msg.contains("官方"), "{msg}");
    }

    #[test]
    fn each_capability_resolves_its_own_endpoint() {
        let mut config = ProjectConfig::default();
        config.slot_mut(Capability::Image).provider = ProviderId::Google;
        let creds = creds_with_all();
        let factory = ProviderFactory::new(&config, &creds);

        let image = factory.image().unwrap();
        assert_eq!(image.endpoint().provider, ProviderId::Google);
        assert!(image
            .endpoint()
            .base_url
            .starts_with("https://generativelanguage.googleapis.com"));

        // 改生图不影响对话
        let chat = factory.chat().unwrap();
        assert_eq!(chat.endpoint().provider, ProviderId::OpenAi);
        assert_eq!(chat.endpoint().base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn credentials_keep_official_and_custom_apart() {
        let cap = Capability::Chat;
        let mut c = Credentials::default();
        c.set(cap, ProviderId::OpenAi, EndpointMode::Official, "official");
        c.set(cap, ProviderId::OpenAi, EndpointMode::Custom, "custom");
        assert_eq!(
            c.get(cap, ProviderId::OpenAi, EndpointMode::Official),
            Some("official")
        );
        assert_eq!(
            c.get(cap, ProviderId::OpenAi, EndpointMode::Custom),
            Some("custom")
        );
        c.set(cap, ProviderId::OpenAi, EndpointMode::Custom, "   ");
        assert!(!c.has(cap, ProviderId::OpenAi, EndpointMode::Custom));
    }

    #[test]
    fn capabilities_do_not_share_credentials() {
        let config = ProjectConfig::default();
        let mut creds = Credentials::default();
        creds.set(Capability::Chat, ProviderId::OpenAi, EndpointMode::Official, "k");
        let factory = ProviderFactory::new(&config, &creds);

        assert!(factory.chat().is_ok());
        // 同一家、同一模式，但生图那格没填就是没填
        let err = factory
            .image()
            .err()
            .expect("image key missing")
            .to_string();
        assert!(err.contains("图像"), "{err}");
    }
}
