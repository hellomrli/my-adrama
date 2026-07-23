//! Global app settings (API keys etc.) persisted under the user config dir.
//! Keys are stored only in this local file (or process env), never in project.toml.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub openai_api_key: String,
    #[serde(default)]
    pub gemini_api_key: String,
    #[serde(default)]
    pub xai_api_key: String,
    #[serde(default)]
    pub custom_api_key: String,
    /// Last opened project path
    #[serde(default)]
    pub last_project: Option<PathBuf>,
    /// Recently opened projects (newest first, max 12)
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
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
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

    /// Push keys into process environment so stage runners can read them.
    pub fn apply_to_env(&self) {
        set_env_if_nonempty("OPENAI_API_KEY", &self.openai_api_key);
        set_env_if_nonempty("GEMINI_API_KEY", &self.gemini_api_key);
        set_env_if_nonempty("XAI_API_KEY", &self.xai_api_key);
        set_env_if_nonempty("ADRAMA_CUSTOM_API_KEY", &self.custom_api_key);
    }

    pub fn merge_from_env(&mut self) {
        if self.openai_api_key.trim().is_empty() {
            if let Ok(v) = std::env::var("OPENAI_API_KEY") {
                self.openai_api_key = v;
            }
        }
        if self.gemini_api_key.trim().is_empty() {
            if let Ok(v) = std::env::var("GEMINI_API_KEY") {
                self.gemini_api_key = v;
            } else if let Ok(v) = std::env::var("GOOGLE_API_KEY") {
                self.gemini_api_key = v;
            }
        }
        if self.xai_api_key.trim().is_empty() {
            if let Ok(v) = std::env::var("XAI_API_KEY") {
                self.xai_api_key = v;
            } else if let Ok(v) = std::env::var("GROK_API_KEY") {
                self.xai_api_key = v;
            }
        }
        if self.custom_api_key.trim().is_empty() {
            if let Ok(v) = std::env::var("ADRAMA_CUSTOM_API_KEY") {
                self.custom_api_key = v;
            }
        }
    }

    pub fn remember_project(&mut self, path: PathBuf) {
        self.last_project = Some(path.clone());
        self.recent_projects.retain(|p| p != &path);
        self.recent_projects.insert(0, path);
        self.recent_projects.truncate(12);
    }
}

fn set_env_if_nonempty(key: &str, value: &str) {
    let t = value.trim();
    if !t.is_empty() {
        std::env::set_var(key, t);
    }
}
