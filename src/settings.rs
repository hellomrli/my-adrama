//! Global app settings (API keys etc.) persisted under the user config dir.
//! Keys are stored only in this local file (or process env), never in project.toml.
//!
//! Each vendor has **official** and **custom** keys so users can switch mode without
//! overwriting the other credential.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    // --- OpenAI / Image2 ---
    #[serde(default)]
    pub openai_official_key: String,
    #[serde(default)]
    pub openai_custom_key: String,
    /// legacy single key (migrated on load)
    #[serde(default, alias = "openai_api_key")]
    pub openai_api_key_legacy: String,

    // --- Google / Gemini / Veo ---
    #[serde(default)]
    pub google_official_key: String,
    #[serde(default)]
    pub google_custom_key: String,
    #[serde(default, alias = "gemini_api_key")]
    pub gemini_api_key_legacy: String,

    // --- xAI / Grok ---
    #[serde(default)]
    pub xai_official_key: String,
    #[serde(default)]
    pub xai_custom_key: String,
    #[serde(default, alias = "xai_api_key")]
    pub xai_api_key_legacy: String,

    // --- generic custom endpoint (legacy / extra) ---
    #[serde(default)]
    pub custom_api_key: String,

    #[serde(default)]
    pub last_project: Option<PathBuf>,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
}

impl AppSettings {
    pub fn config_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("ADRAMA_CONFIG_DIR") {
            return PathBuf::from(dir).join("settings.json");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("adrama")
                .join("settings.json");
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("adrama").join("settings.json");
        }
        PathBuf::from("adrama-settings.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if !path.exists() {
            return Self::default();
        }
        match fs::read_to_string(&path) {
            Ok(s) => {
                let mut settings: Self = serde_json::from_str(&s).unwrap_or_default();
                settings.migrate_legacy();
                settings
            }
            Err(_) => Self::default(),
        }
    }

    fn migrate_legacy(&mut self) {
        if self.openai_official_key.trim().is_empty() && !self.openai_api_key_legacy.trim().is_empty()
        {
            self.openai_official_key = self.openai_api_key_legacy.clone();
        }
        if self.google_official_key.trim().is_empty() && !self.gemini_api_key_legacy.trim().is_empty()
        {
            self.google_official_key = self.gemini_api_key_legacy.clone();
        }
        if self.xai_official_key.trim().is_empty() && !self.xai_api_key_legacy.trim().is_empty() {
            self.xai_official_key = self.xai_api_key_legacy.clone();
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
        let s = serde_json::to_string_pretty(self)?;
        fs::write(&path, s).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Push all keys into process env for stage runners.
    /// Official keys use standard names; custom keys use ADRAMA_*_CUSTOM_KEY.
    pub fn apply_to_env(&self) {
        set_env("OPENAI_API_KEY", &self.openai_official_key);
        set_env("ADRAMA_OPENAI_CUSTOM_KEY", &self.openai_custom_key);
        set_env("GEMINI_API_KEY", &self.google_official_key);
        set_env("ADRAMA_GOOGLE_CUSTOM_KEY", &self.google_custom_key);
        set_env("XAI_API_KEY", &self.xai_official_key);
        set_env("ADRAMA_XAI_CUSTOM_KEY", &self.xai_custom_key);
        set_env("ADRAMA_CUSTOM_API_KEY", &self.custom_api_key);
    }

    pub fn merge_from_env(&mut self) {
        fill_if_empty(&mut self.openai_official_key, &["OPENAI_API_KEY"]);
        fill_if_empty(
            &mut self.openai_custom_key,
            &["ADRAMA_OPENAI_CUSTOM_KEY"],
        );
        fill_if_empty(
            &mut self.google_official_key,
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        );
        fill_if_empty(
            &mut self.google_custom_key,
            &["ADRAMA_GOOGLE_CUSTOM_KEY"],
        );
        fill_if_empty(
            &mut self.xai_official_key,
            &["XAI_API_KEY", "GROK_API_KEY"],
        );
        fill_if_empty(&mut self.xai_custom_key, &["ADRAMA_XAI_CUSTOM_KEY"]);
        fill_if_empty(
            &mut self.custom_api_key,
            &["ADRAMA_CUSTOM_API_KEY", "CUSTOM_API_KEY"],
        );
    }

    pub fn remember_project(&mut self, path: PathBuf) {
        self.last_project = Some(path.clone());
        self.recent_projects.retain(|p| p != &path);
        self.recent_projects.insert(0, path);
        self.recent_projects.truncate(12);
    }
}

fn set_env(key: &str, value: &str) {
    let t = value.trim();
    if t.is_empty() {
        // Don't wipe env if UI field empty — allow external env-only setup
        return;
    }
    std::env::set_var(key, t);
}

fn fill_if_empty(target: &mut String, keys: &[&str]) {
    if !target.trim().is_empty() {
        return;
    }
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            let t = v.trim();
            if !t.is_empty() {
                *target = t.to_string();
                return;
            }
        }
    }
}
