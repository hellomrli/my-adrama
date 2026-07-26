//! User-level settings: API keys, recent projects, UI preferences.
//!
//! Stored outside any project so keys never end up in a shared repo, and
//! written with owner-only permissions on unix.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::model::{EndpointMode, ProviderId};
use crate::providers::Credentials;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    /// `"openai.official"` → key. A map keeps adding providers cheap.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    #[serde(default)]
    pub last_project: Option<PathBuf>,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    #[serde(default)]
    pub ui: UiPrefs,

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
            self.keys.entry(slot(id, mode)).or_insert(value);
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

    pub fn key(&self, id: ProviderId, mode: EndpointMode) -> &str {
        self.keys.get(&slot(id, mode)).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn set_key(&mut self, id: ProviderId, mode: EndpointMode, value: impl Into<String>) {
        let value = value.into();
        if value.trim().is_empty() {
            self.keys.remove(&slot(id, mode));
        } else {
            self.keys.insert(slot(id, mode), value.trim().to_string());
        }
    }

    /// Credentials for a job: stored keys, with environment variables filling
    /// any gaps (so `OPENAI_API_KEY=… adrama parse` still works).
    pub fn credentials(&self) -> Credentials {
        let mut creds = Credentials::default();
        for id in ProviderId::ALL {
            for mode in EndpointMode::ALL {
                let value = self.key(id, mode);
                if !value.is_empty() {
                    creds.set(id, mode, value);
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

    pub fn forget_project(&mut self, path: &PathBuf) {
        self.recent_projects.retain(|p| p != path);
        if self.last_project.as_ref() == Some(path) {
            self.last_project = None;
        }
    }
}

fn slot(id: ProviderId, mode: EndpointMode) -> String {
    format!(
        "{}.{}",
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

        assert_eq!(settings.key(ProviderId::OpenAi, EndpointMode::Official), "sk-old");
        assert_eq!(settings.key(ProviderId::Google, EndpointMode::Custom), "proxy-key");
        assert_eq!(settings.recent_projects.len(), 1);

        // Re-serializing drops the legacy fields.
        let text = serde_json::to_string(&settings).unwrap();
        assert!(!text.contains("openai_api_key"));
        assert!(text.contains("openai.official"));
    }

    #[test]
    fn official_and_custom_keys_are_independent() {
        let mut s = AppSettings::default();
        s.set_key(ProviderId::OpenAi, EndpointMode::Official, "a");
        s.set_key(ProviderId::OpenAi, EndpointMode::Custom, "b");
        assert_eq!(s.key(ProviderId::OpenAi, EndpointMode::Official), "a");
        assert_eq!(s.key(ProviderId::OpenAi, EndpointMode::Custom), "b");
        s.set_key(ProviderId::OpenAi, EndpointMode::Custom, "");
        assert_eq!(s.key(ProviderId::OpenAi, EndpointMode::Custom), "");
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
