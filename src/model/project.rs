//! The project directory: layout, load/save, and derived paths.
//!
//! Everything the pipeline knows lives on disk as readable files, so a user can
//! hand-edit a prompt or drop in their own image and re-run. This type owns
//! that layout and nothing else — no credentials, no HTTP, no UI.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::breakdown::Breakdown;
use super::config::{AspectRatio, ProjectConfig};
use super::state::ProjectState;

pub const CONFIG_FILE: &str = "project.toml";
pub const STATE_FILE: &str = "state.json";
pub const BREAKDOWN_FILE: &str = "parsed/breakdown.json";
pub const SCRIPT_EXTENSIONS: &[&str] = &["md", "txt", "fountain"];

/// Asset families, in the order they are generated and displayed.
pub const ASSET_KINDS: [AssetKind; 4] = [
    AssetKind::Character,
    AssetKind::Costume,
    AssetKind::Prop,
    AssetKind::Location,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetKind {
    Character,
    Costume,
    Prop,
    Location,
}

impl AssetKind {
    /// Directory name under `assets/`.
    pub fn dir(self) -> &'static str {
        match self {
            AssetKind::Character => "characters",
            AssetKind::Costume => "costumes",
            AssetKind::Prop => "props",
            AssetKind::Location => "locations",
        }
    }

    /// Value stored in `meta.json`.
    pub fn tag(self) -> &'static str {
        match self {
            AssetKind::Character => "character",
            AssetKind::Costume => "costume",
            AssetKind::Prop => "prop",
            AssetKind::Location => "location",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Character => "角色",
            AssetKind::Costume => "服装",
            AssetKind::Prop => "道具",
            AssetKind::Location => "场景",
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
    /// Create a new project directory tree.
    pub fn create(path: &Path, name: &str, style: &str, aspect: AspectRatio) -> Result<Self> {
        if path.exists() && path.read_dir()?.next().is_some() {
            bail!("目录非空：{}", path.display());
        }
        fs::create_dir_all(path).with_context(|| format!("创建目录 {}", path.display()))?;
        for sub in Self::skeleton() {
            fs::create_dir_all(path.join(sub))?;
        }

        let proj = Self {
            root: normalize_root(path),
            config: ProjectConfig::new(name, style, aspect),
            state: ProjectState::default(),
        };
        proj.save_config()?;
        proj.save_state()?;
        Ok(proj)
    }

    fn skeleton() -> impl Iterator<Item = String> {
        let mut dirs = vec![
            "script".to_string(),
            "parsed".into(),
            "storyboard".into(),
            "video".into(),
            "voice".into(),
        ];
        dirs.extend(ASSET_KINDS.iter().map(|k| format!("assets/{}", k.dir())));
        dirs.into_iter()
    }

    pub fn open(path: &Path) -> Result<Self> {
        let root = normalize_root(
            &fs::canonicalize(path)
                .with_context(|| format!("无法打开项目：{}", path.display()))?,
        );
        let config_path = root.join(CONFIG_FILE);
        if !config_path.exists() {
            bail!("不是 adrama 项目（缺少 {CONFIG_FILE}）：{}", root.display());
        }
        let config_str = fs::read_to_string(&config_path)?;
        let mut config: ProjectConfig = toml::from_str(&config_str)
            .with_context(|| format!("{CONFIG_FILE} 格式错误：{}", config_path.display()))?;
        config.normalize();

        let state_path = root.join(STATE_FILE);
        let state = if state_path.exists() {
            serde_json::from_str(&fs::read_to_string(&state_path)?).unwrap_or_default()
        } else {
            ProjectState::default()
        };

        Ok(Self {
            root,
            config,
            state,
        })
    }

    /// Is `path` an adrama project root?
    pub fn is_project(path: &Path) -> bool {
        path.join(CONFIG_FILE).is_file()
    }

    pub fn save_config(&self) -> Result<()> {
        let text = toml::to_string_pretty(&self.config)?;
        write_atomic(&self.root.join(CONFIG_FILE), text.as_bytes())
    }

    pub fn save_state(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.state)?;
        write_atomic(&self.root.join(STATE_FILE), text.as_bytes())
    }

    // --- paths -------------------------------------------------------------

    pub fn script_dir(&self) -> PathBuf {
        self.root.join("script")
    }

    pub fn breakdown_path(&self) -> PathBuf {
        self.root.join(BREAKDOWN_FILE)
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn asset_dir(&self, kind: AssetKind, id: &str) -> PathBuf {
        self.assets_dir().join(kind.dir()).join(id)
    }

    pub fn storyboard_dir(&self) -> PathBuf {
        self.root.join("storyboard")
    }

    /// 第 i 帧（1 起）的文件路径：`<shot>_k1.png` … `<shot>_kN.png`。
    pub fn storyboard_keyframe(&self, shot_id: &str, i: u32) -> PathBuf {
        self.storyboard_dir().join(format!("{shot_id}_k{i}.png"))
    }

    /// 首帧。兼容旧命名 `<shot>.png`（视作第 1 帧）。
    pub fn storyboard_image(&self, shot_id: &str) -> PathBuf {
        let k1 = self.storyboard_keyframe(shot_id, 1);
        if k1.is_file() {
            return k1;
        }
        let legacy = self.storyboard_dir().join(format!("{shot_id}.png"));
        if legacy.is_file() {
            legacy
        } else {
            k1
        }
    }

    /// 已存在的全部关键帧，按序号排列（旧命名视作第 1 帧）。
    pub fn storyboard_keyframes(&self, shot_id: &str) -> Vec<PathBuf> {
        let mut frames = Vec::new();
        let legacy = self.storyboard_dir().join(format!("{shot_id}.png"));
        let k1 = self.storyboard_keyframe(shot_id, 1);
        if k1.is_file() {
            frames.push(k1);
        } else if legacy.is_file() {
            frames.push(legacy);
        }
        for i in 2..=16u32 {
            let p = self.storyboard_keyframe(shot_id, i);
            if p.is_file() {
                frames.push(p);
            }
        }
        frames
    }

    /// 末帧（仅当存在第 2 帧及以后时才有）。
    pub fn storyboard_last(&self, shot_id: &str) -> Option<PathBuf> {
        let frames = self.storyboard_keyframes(shot_id);
        if frames.len() >= 2 {
            frames.last().cloned()
        } else {
            None
        }
    }

    pub fn storyboard_meta(&self, shot_id: &str) -> PathBuf {
        self.storyboard_dir().join(format!("{shot_id}.json"))
    }

    pub fn video_dir(&self) -> PathBuf {
        self.root.join("video")
    }

    pub fn video_clip(&self, shot_id: &str) -> PathBuf {
        self.video_dir().join(format!("{shot_id}.mp4"))
    }

    pub fn video_meta(&self, shot_id: &str) -> PathBuf {
        self.video_dir().join(format!("{shot_id}.json"))
    }

    pub fn final_cut(&self) -> PathBuf {
        self.video_dir().join("final.mp4")
    }

    pub fn voice_dir(&self) -> PathBuf {
        self.root.join("voice")
    }

    pub fn voice_clip(&self, shot_id: &str) -> PathBuf {
        self.voice_dir().join(format!("{shot_id}.mp3"))
    }

    /// 已存在的配音文件：云端 API 是 mp3，本地 Piper 是 wav。
    pub fn find_voice_clip(&self, shot_id: &str) -> Option<PathBuf> {
        for ext in ["mp3", "wav"] {
            let p = self.voice_dir().join(format!("{shot_id}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    pub fn voice_meta(&self, shot_id: &str) -> PathBuf {
        self.voice_dir().join(format!("{shot_id}.json"))
    }

    pub fn subtitles_path(&self) -> PathBuf {
        self.video_dir().join("subtitles.srt")
    }

    // --- documents ---------------------------------------------------------

    pub fn load_breakdown(&self) -> Result<Breakdown> {
        let path = self.breakdown_path();
        if !path.exists() {
            bail!("尚未生成 breakdown.json，请先运行「解析」阶段");
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).with_context(|| format!("解析 {} 失败", path.display()))
    }

    pub fn save_breakdown(&self, breakdown: &Breakdown) -> Result<()> {
        let path = self.breakdown_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, serde_json::to_string_pretty(breakdown)?.as_bytes())
    }

    /// First script file in `script/`, alphabetically.
    pub fn find_script(&self) -> Option<PathBuf> {
        let dir = self.script_dir();
        let mut candidates: Vec<PathBuf> = fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| has_extension(p, SCRIPT_EXTENSIONS))
            .collect();
        candidates.sort();
        candidates.into_iter().next()
    }

    pub fn read_script(&self) -> Result<(PathBuf, String)> {
        let path = self
            .find_script()
            .with_context(|| format!("{} 下没有剧本文件", self.script_dir().display()))?;
        let text = fs::read_to_string(&path)
            .with_context(|| format!("读取剧本 {}", path.display()))?;
        Ok((path, text))
    }

    /// Write the script, creating `script/script.md` when none exists yet.
    pub fn write_script(&self, text: &str) -> Result<PathBuf> {
        let path = self
            .find_script()
            .unwrap_or_else(|| self.script_dir().join("script.md"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, text.as_bytes())?;
        Ok(path)
    }

    /// Copy an external script into the project.
    pub fn import_script(&self, source: &Path) -> Result<PathBuf> {
        if !source.is_file() {
            bail!("剧本文件不存在：{}", source.display());
        }
        let dir = self.script_dir();
        fs::create_dir_all(&dir)?;
        let name = source
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("script.md");
        let dest = dir.join(name);
        fs::copy(source, &dest)
            .with_context(|| format!("复制 {} → {}", source.display(), dest.display()))?;
        Ok(dest)
    }

}

/// Write via a temp file + rename so a crash mid-write cannot truncate state.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("new")
    ));
    fs::write(&tmp, bytes).with_context(|| format!("写入 {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
    Ok(())
}

pub fn has_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.iter().any(|want| e.eq_ignore_ascii_case(want)))
        .unwrap_or(false)
}

/// Strip Windows' `\\?\` verbatim prefix, which `canonicalize` adds and which
/// confuses external tools (ffmpeg) and looks alarming in the UI.
fn normalize_root(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_open_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        let created = Project::create(&root, "demo", "noir", AspectRatio::Portrait).unwrap();
        assert!(root.join(CONFIG_FILE).is_file());
        assert!(root.join("assets/characters").is_dir());

        let opened = Project::open(&root).unwrap();
        assert_eq!(opened.config.name, created.config.name);
        assert_eq!(opened.config.aspect, AspectRatio::Portrait);
        assert!(Project::is_project(&root));
    }

    #[test]
    fn create_refuses_nonempty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("busy");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("stray.txt"), b"hi").unwrap();
        assert!(Project::create(&root, "busy", "s", AspectRatio::Landscape).is_err());
    }

    #[test]
    fn script_write_then_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();
        assert!(proj.find_script().is_none());

        let path = proj.write_script("场景一\n内景").unwrap();
        assert_eq!(path.file_name().unwrap(), "script.md");
        let (_, text) = proj.read_script().unwrap();
        assert!(text.contains("内景"));
    }
}
