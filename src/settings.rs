//! User-level settings: API keys, recent projects, UI preferences.
//!
//! Stored outside any project so keys never end up in a shared repo, and
//! written with owner-only permissions on unix.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::model::{Capability, EndpointMode, ProviderId};
use crate::providers::Credentials;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    /// `"chat.openai.official"` → key。按能力隔离：同一家服务商在对话和生图上
    /// 可能是两个中转、两把密钥。
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    #[serde(default)]
    pub last_project: Option<PathBuf>,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    #[serde(default)]
    pub ui: UiPrefs,
    /// `"chat.openai.official"` → 上次从该端点拉取到的模型 ID 列表。
    #[serde(default)]
    pub model_cache: BTreeMap<String, Vec<String>>,

    // --- legacy 0.1.x fields, migrated on load then dropped on save ---
    #[serde(default, skip_serializing)]
    openai_official_key: String,
    #[serde(default, skip_serializing)]
    openai_custom_key: String,
    #[serde(default, skip_serializing, alias = "openai_api_key")]
    openai_api_key_legacy: String,
    #[serde(default, skip_serializing)]
    google_official_key: String,
    #[serde(default, skip_serializing)]
    google_custom_key: String,
    #[serde(default, skip_serializing, alias = "gemini_api_key")]
    gemini_api_key_legacy: String,
    #[serde(default, skip_serializing)]
    xai_official_key: String,
    #[serde(default, skip_serializing)]
    xai_custom_key: String,
    #[serde(default, skip_serializing, alias = "xai_api_key")]
    xai_api_key_legacy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiPrefs {
    /// Bottom console drawer open?
    #[serde(default = "yes")]
    pub console_open: bool,
    /// Grid thumbnail edge length in points.
    #[serde(default = "default_thumb")]
    pub thumbnail_size: f32,
    /// Remembered dry-run toggle.
    #[serde(default)]
    pub dry_run: bool,
    /// Screen to reopen on launch.
    #[serde(default)]
    pub last_view: Option<String>,
    /// Check GitHub for a newer release on startup (once a day).
    #[serde(default = "yes")]
    pub auto_check_updates: bool,
    /// Unix seconds of the last check, so we do not ask on every launch.
    #[serde(default)]
    pub last_update_check: u64,
}

fn yes() -> bool {
    true
}

fn default_thumb() -> f32 {
    168.0
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            console_open: true,
            thumbnail_size: default_thumb(),
            dry_run: false,
            last_view: None,
            auto_check_updates: true,
            last_update_check: 0,
        }
    }
}

impl AppSettings {
    pub fn config_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("ADRAMA_CONFIG_DIR") {
            return PathBuf::from(dir).join("settings.json");
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("adrama").join("settings.json");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("adrama")
                .join("settings.json");
        }
        PathBuf::from("adrama-settings.json")
    }

    /// 运行日志的位置（图形界面看不到 stderr，出问题时这是唯一线索）。
    pub fn log_path() -> PathBuf {
        Self::config_path().with_file_name("adrama.log")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let mut settings: Self = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        settings.migrate_legacy();
        settings
    }

    fn migrate_legacy(&mut self) {
        // 0.2.x：密钥按「服务商.模式」存，没有能力维度。复制给三种能力，
        // 之后各自独立。
        let shared: Vec<(String, String)> = self
            .keys
            .iter()
            .filter(|(k, _)| k.split('.').count() == 2)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (old_key, value) in shared {
            for cap in Capability::ALL {
                self.keys
                    .entry(format!("{}.{old_key}", cap.as_str()))
                    .or_insert_with(|| value.clone());
            }
            self.keys.remove(&old_key);
        }
        let stale: Vec<String> = self
            .model_cache
            .keys()
            .filter(|k| k.split('.').count() == 2)
            .cloned()
            .collect();
        for key in stale {
            if let Some(models) = self.model_cache.remove(&key) {
                for cap in Capability::ALL {
                    self.model_cache
                        .entry(format!("{}.{key}", cap.as_str()))
                        .or_insert_with(|| models.clone());
                }
            }
        }

        let legacy = [
            (ProviderId::OpenAi, EndpointMode::Official, std::mem::take(&mut self.openai_official_key)),
            (ProviderId::OpenAi, EndpointMode::Official, std::mem::take(&mut self.openai_api_key_legacy)),
            (ProviderId::OpenAi, EndpointMode::Custom, std::mem::take(&mut self.openai_custom_key)),
            (ProviderId::Google, EndpointMode::Official, std::mem::take(&mut self.google_official_key)),
            (ProviderId::Google, EndpointMode::Official, std::mem::take(&mut self.gemini_api_key_legacy)),
            (ProviderId::Google, EndpointMode::Custom, std::mem::take(&mut self.google_custom_key)),
            (ProviderId::Xai, EndpointMode::Official, std::mem::take(&mut self.xai_official_key)),
            (ProviderId::Xai, EndpointMode::Official, std::mem::take(&mut self.xai_api_key_legacy)),
            (ProviderId::Xai, EndpointMode::Custom, std::mem::take(&mut self.xai_custom_key)),
        ];
        for (id, mode, value) in legacy {
            if value.trim().is_empty() {
                continue;
            }
            for cap in Capability::ALL {
                self.keys
                    .entry(slot(cap, id, mode))
                    .or_insert_with(|| value.clone());
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录 {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("写入 {}", path.display()))?;
        restrict_permissions(&path);
        Ok(())
    }

    pub fn key(&self, cap: Capability, id: ProviderId, mode: EndpointMode) -> &str {
        self.keys
            .get(&slot(cap, id, mode))
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    pub fn set_key(
        &mut self,
        cap: Capability,
        id: ProviderId,
        mode: EndpointMode,
        value: impl Into<String>,
    ) {
        let value = value.into();
        if value.trim().is_empty() {
            self.keys.remove(&slot(cap, id, mode));
        } else {
            self.keys
                .insert(slot(cap, id, mode), value.trim().to_string());
        }
    }

    /// Credentials for a job: stored keys, with environment variables filling
    /// any gaps (so `OPENAI_API_KEY=… adrama parse` still works).
    /// Model ids last fetched from this endpoint.
    pub fn known_models(&self, cap: Capability, id: ProviderId, mode: EndpointMode) -> &[String] {
        self.model_cache
            .get(&slot(cap, id, mode))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn set_known_models(
        &mut self,
        cap: Capability,
        id: ProviderId,
        mode: EndpointMode,
        models: Vec<String>,
    ) {
        if models.is_empty() {
            self.model_cache.remove(&slot(cap, id, mode));
        } else {
            self.model_cache.insert(slot(cap, id, mode), models);
        }
    }

    pub fn credentials(&self) -> Credentials {
        let mut creds = Credentials::default();
        for cap in Capability::ALL {
            for id in ProviderId::ALL {
                for mode in EndpointMode::ALL {
                    let value = self.key(cap, id, mode);
                    if !value.is_empty() {
                        creds.set(cap, id, mode, value);
                    }
                }
            }
        }
        creds.fill_from_env();
        creds
    }

    pub fn remember_project(&mut self, path: PathBuf) {
        self.last_project = Some(path.clone());
        self.recent_projects.retain(|p| p != &path);
        self.recent_projects.insert(0, path);
        self.recent_projects.truncate(12);
    }

    /// Should we look for a new release now?
    pub fn update_check_due(&self) -> bool {
        if !self.ui.auto_check_updates {
            return false;
        }
        now_secs().saturating_sub(self.ui.last_update_check) >= crate::update::CHECK_INTERVAL.as_secs()
    }

    pub fn mark_update_checked(&mut self) {
        self.ui.last_update_check = now_secs();
    }

    pub fn forget_project(&mut self, path: &PathBuf) {
        self.recent_projects.retain(|p| p != path);
        if self.last_project.as_ref() == Some(path) {
            self.last_project = None;
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn slot(cap: Capability, id: ProviderId, mode: EndpointMode) -> String {
    format!(
        "{}.{}.{}",
        cap.as_str(),
        id.as_str(),
        match mode {
            EndpointMode::Official => "official",
            EndpointMode::Custom => "custom",
        }
    )
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_keys_migrate_into_the_map() {
        let json = r#"{
            "openai_api_key": "sk-old",
            "google_custom_key": "proxy-key",
            "recent_projects": ["/tmp/a"]
        }"#;
        let mut settings: AppSettings = serde_json::from_str(json).unwrap();
        settings.migrate_legacy();

        // 0.1.x 的单把密钥会复制给三种能力，之后各自独立
        for cap in Capability::ALL {
            assert_eq!(settings.key(cap, ProviderId::OpenAi, EndpointMode::Official), "sk-old");
            assert_eq!(settings.key(cap, ProviderId::Google, EndpointMode::Custom), "proxy-key");
        }
        assert_eq!(settings.recent_projects.len(), 1);

        // Re-serializing drops the legacy fields.
        let text = serde_json::to_string(&settings).unwrap();
        assert!(!text.contains("openai_api_key"));
        assert!(text.contains("chat.openai.official"));
    }

    #[test]
    fn official_and_custom_keys_are_independent() {
        let mut s = AppSettings::default();
        let cap = Capability::Chat;
        s.set_key(cap, ProviderId::OpenAi, EndpointMode::Official, "a");
        s.set_key(cap, ProviderId::OpenAi, EndpointMode::Custom, "b");
        assert_eq!(s.key(cap, ProviderId::OpenAi, EndpointMode::Official), "a");
        assert_eq!(s.key(cap, ProviderId::OpenAi, EndpointMode::Custom), "b");
        s.set_key(cap, ProviderId::OpenAi, EndpointMode::Custom, "");
        assert_eq!(s.key(cap, ProviderId::OpenAi, EndpointMode::Custom), "");
    }

    #[test]
    fn capabilities_do_not_share_keys() {
        let mut s = AppSettings::default();
        s.set_key(Capability::Chat, ProviderId::OpenAi, EndpointMode::Custom, "chat-relay-key");
        // 同一家、同一模式，但生图那格仍然是空的
        assert_eq!(
            s.key(Capability::Image, ProviderId::OpenAi, EndpointMode::Custom),
            ""
        );
        s.set_key(Capability::Image, ProviderId::OpenAi, EndpointMode::Custom, "image-relay-key");
        assert_eq!(
            s.key(Capability::Chat, ProviderId::OpenAi, EndpointMode::Custom),
            "chat-relay-key"
        );
    }

    #[test]
    fn v0_2_keys_migrate_to_every_capability() {
        let json = r#"{ "keys": { "openai.official": "sk-shared" } }"#;
        let mut settings: AppSettings = serde_json::from_str(json).unwrap();
        settings.migrate_legacy();
        for cap in Capability::ALL {
            assert_eq!(settings.key(cap, ProviderId::OpenAi, EndpointMode::Official), "sk-shared");
        }
        assert!(!settings.keys.contains_key("openai.official"));
    }

    #[test]
    fn recent_projects_deduplicate_most_recent_first() {
        let mut s = AppSettings::default();
        s.remember_project(PathBuf::from("/a"));
        s.remember_project(PathBuf::from("/b"));
        s.remember_project(PathBuf::from("/a"));
        assert_eq!(s.recent_projects, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(s.last_project, Some(PathBuf::from("/a")));
    }
}
