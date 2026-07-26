//! Derived view of everything the pipeline is supposed to produce.
//!
//! The old GUI listed whatever image files happened to exist and guessed their
//! status by trimming filename suffixes. Instead we enumerate the *expected*
//! work items from the breakdown and merge in what is on disk, so a stage can
//! show "12 项，7 已生成，1 失败，4 待生成" before anything has run.

use std::fs;
use std::path::{Path, PathBuf};

use super::breakdown::{AssetMeta, Breakdown, StoryboardMeta, VideoMeta};
use super::project::{has_extension, AssetKind, Project};
use super::state::{ItemStatus, Stage};

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Asset(AssetKind),
    Storyboard,
    Video,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Asset(k) => k.label(),
            ItemKind::Storyboard => "分镜",
            ItemKind::Video => "片段",
        }
    }
}

/// One reviewable unit of generated work.
#[derive(Debug, Clone)]
pub struct ItemView {
    pub kind: ItemKind,
    /// Asset id or shot id — what `--only` / `--shot` expects.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub status: ItemStatus,
    /// Preview images, first one is the primary thumbnail.
    pub images: Vec<PathBuf>,
    /// Playable media (video clips).
    pub media: Option<PathBuf>,
    pub prompt: String,
    pub references: Vec<String>,
    pub error: Option<String>,
    pub duration_secs: Option<u32>,
    /// Scene number, for grouping shots.
    pub scene: Option<u32>,
}

impl ItemView {
    pub fn thumbnail(&self) -> Option<&Path> {
        self.images.first().map(|p| p.as_path())
    }

}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub ready: usize,
    pub failed: usize,
    pub pending: usize,
}

impl Counts {
    pub fn of(items: &[ItemView]) -> Self {
        let mut c = Counts {
            total: items.len(),
            ..Default::default()
        };
        for item in items {
            match item.status {
                ItemStatus::Done | ItemStatus::Approved => c.ready += 1,
                ItemStatus::Failed => c.failed += 1,
                _ => c.pending += 1,
            }
        }
        c
    }

    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.ready as f32 / self.total as f32
        }
    }

    pub fn summary(&self) -> String {
        if self.total == 0 {
            return "暂无条目".into();
        }
        let mut s = format!("{}/{} 已生成", self.ready, self.total);
        if self.failed > 0 {
            s.push_str(&format!(" · {} 失败", self.failed));
        }
        s
    }
}

/// Snapshot of a project's work items. Cheap enough to rebuild on demand, but
/// built off the UI thread all the same.
#[derive(Debug, Clone, Default)]
pub struct ProjectIndex {
    pub assets: Vec<ItemView>,
    pub storyboard: Vec<ItemView>,
    pub videos: Vec<ItemView>,
    pub final_cut: Option<PathBuf>,
}

impl ProjectIndex {
    pub fn build(project: &Project, breakdown: Option<&Breakdown>) -> Self {
        let assets = breakdown.map(|bd| asset_items(project, bd)).unwrap_or_default();
        let storyboard = breakdown
            .map(|bd| storyboard_items(project, bd))
            .unwrap_or_default();
        let videos = breakdown.map(|bd| video_items(project, bd)).unwrap_or_default();
        let final_cut = project.final_cut();

        Self {
            assets,
            storyboard,
            videos,
            final_cut: final_cut.is_file().then_some(final_cut),
        }
    }

    pub fn items(&self, stage: Stage) -> &[ItemView] {
        match stage {
            Stage::Assets => &self.assets,
            Stage::Storyboard => &self.storyboard,
            Stage::Video => &self.videos,
            Stage::Parse => &[],
        }
    }

    pub fn counts(&self, stage: Stage) -> Counts {
        Counts::of(self.items(stage))
    }

    pub fn find(&self, stage: Stage, id: &str) -> Option<&ItemView> {
        self.items(stage).iter().find(|i| i.id == id)
    }
}

fn asset_items(project: &Project, bd: &Breakdown) -> Vec<ItemView> {
    let mut items = Vec::new();

    for ch in &bd.characters {
        items.push(asset_item(
            project,
            AssetKind::Character,
            &ch.id,
            &ch.name,
            &truncate(&ch.appearance, 70),
        ));
    }
    for c in &bd.costumes {
        items.push(asset_item(
            project,
            AssetKind::Costume,
            &c.id,
            &c.name,
            &truncate(&c.description, 70),
        ));
    }
    for p in &bd.props {
        items.push(asset_item(
            project,
            AssetKind::Prop,
            &p.id,
            &p.name,
            &truncate(&p.description, 70),
        ));
    }
    for l in &bd.locations {
        items.push(asset_item(
            project,
            AssetKind::Location,
            &l.id,
            &l.name,
            &truncate(&l.description, 70),
        ));
    }

    items
}

fn asset_item(
    project: &Project,
    kind: AssetKind,
    id: &str,
    name: &str,
    subtitle: &str,
) -> ItemView {
    let dir = project.asset_dir(kind, id);
    let meta: Option<AssetMeta> = read_json(&dir.join("meta.json"));
    let images = collect_files(&dir, IMAGE_EXTENSIONS);
    let prompt_path = dir.join("prompt.txt");
    let prompt = fs::read_to_string(&prompt_path)
        .ok()
        .or_else(|| meta.as_ref().map(|m| m.prompt.clone()))
        .unwrap_or_default();

    let status = derive_status(meta.as_ref().map(|m| m.status), !images.is_empty());

    ItemView {
        kind: ItemKind::Asset(kind),
        id: id.to_string(),
        title: name.to_string(),
        subtitle: subtitle.to_string(),
        status,
        images,
        media: None,
        prompt,
        references: Vec::new(),
        error: meta.and_then(|m| m.error),
        duration_secs: None,
        scene: None,
    }
}

fn storyboard_items(project: &Project, bd: &Breakdown) -> Vec<ItemView> {
    bd.shots
        .iter()
        .map(|shot| {
            let image = project.storyboard_image(&shot.id);
            let meta: Option<StoryboardMeta> = read_json(&project.storyboard_meta(&shot.id));
            let exists = image.is_file();
            let scene = bd.scene(&shot.scene_id).map(|s| s.number);

            ItemView {
                kind: ItemKind::Storyboard,
                id: shot.id.clone(),
                title: format!("{} · {}", shot.id, shot.framing),
                subtitle: truncate(&shot.visual, 80),
                status: derive_status(meta.as_ref().map(|m| m.status), exists),
                images: if exists { vec![image] } else { Vec::new() },
                media: None,
                prompt: meta.as_ref().map(|m| m.prompt.clone()).unwrap_or_default(),
                        references: meta
                    .as_ref()
                    .map(|m| m.reference_assets.clone())
                    .unwrap_or_default(),
                error: meta.and_then(|m| m.error),
                duration_secs: Some(shot.duration_secs),
                scene,
            }
        })
        .collect()
}

fn video_items(project: &Project, bd: &Breakdown) -> Vec<ItemView> {
    bd.shots
        .iter()
        .map(|shot| {
            let clip = project.video_clip(&shot.id);
            let meta: Option<VideoMeta> = read_json(&project.video_meta(&shot.id));
            let exists = clip.is_file();
            let frame = project.storyboard_image(&shot.id);
            let scene = bd.scene(&shot.scene_id).map(|s| s.number);

            let status = match (&meta, exists) {
                (Some(m), false) if m.operation_name.is_some() && m.status == ItemStatus::Generating => {
                    ItemStatus::Generating
                }
                (m, e) => derive_status(m.as_ref().map(|m| m.status), e),
            };

            ItemView {
                kind: ItemKind::Video,
                id: shot.id.clone(),
                title: format!("{} · {}s", shot.id, shot.duration_secs),
                subtitle: truncate(&shot.visual, 80),
                status,
                images: if frame.is_file() { vec![frame] } else { Vec::new() },
                media: exists.then_some(clip),
                prompt: meta.as_ref().map(|m| m.prompt.clone()).unwrap_or_default(),
                        references: meta
                    .as_ref()
                    .and_then(|m| m.operation_name.clone())
                    .map(|op| vec![op])
                    .unwrap_or_default(),
                error: meta.and_then(|m| m.error),
                duration_secs: Some(shot.duration_secs),
                scene,
            }
        })
        .collect()
}

/// Sidecar status wins, but disk truth breaks ties: a `done` sidecar with no
/// file is really pending, and a file with no sidecar is really done.
fn derive_status(recorded: Option<ItemStatus>, has_output: bool) -> ItemStatus {
    match (recorded, has_output) {
        (Some(ItemStatus::Failed), false) => ItemStatus::Failed,
        (Some(ItemStatus::Approved), true) => ItemStatus::Approved,
        (Some(ItemStatus::Generating), false) => ItemStatus::Generating,
        (_, true) => ItemStatus::Done,
        (_, false) => ItemStatus::Pending,
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn collect_files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_extension(p, exts))
        .collect();
    out.sort();
    out
}

pub fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::breakdown::{Character, Shot};
    use crate::model::config::AspectRatio;

    fn demo_breakdown() -> Breakdown {
        Breakdown {
            title: "demo".into(),
            characters: vec![Character {
                id: "char_a".into(),
                name: "阿明".into(),
                appearance: "二十岁，短发".into(),
                costume: String::new(),
                personality: String::new(),
            }],
            shots: vec![Shot {
                id: "shot_1".into(),
                scene_id: "sc1".into(),
                number: 1,
                framing: "medium".into(),
                camera: "static".into(),
                visual: "阿明推门而入".into(),
                dialogue: String::new(),
                sfx: String::new(),
                duration_secs: 5,
                character_ids: vec!["char_a".into()],
                prop_ids: vec![],
                location_id: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn pending_items_appear_before_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();
        let bd = demo_breakdown();

        let index = ProjectIndex::build(&proj, Some(&bd));
        assert_eq!(index.assets.len(), 1);
        assert_eq!(index.assets[0].status, ItemStatus::Pending);
        assert_eq!(index.storyboard.len(), 1);
        assert_eq!(index.videos.len(), 1);
        assert_eq!(index.counts(Stage::Assets).ready, 0);
    }

    #[test]
    fn disk_truth_overrides_stale_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();
        let bd = demo_breakdown();

        // Sidecar claims done, but no image exists yet.
        let dir = proj.asset_dir(AssetKind::Character, "char_a");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("meta.json"),
            serde_json::to_string(&AssetMeta {
                id: "char_a".into(),
                status: ItemStatus::Done,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();

        let index = ProjectIndex::build(&proj, Some(&bd));
        assert_eq!(index.assets[0].status, ItemStatus::Pending);

        fs::write(dir.join("front.png"), b"not-really-a-png").unwrap();
        let index = ProjectIndex::build(&proj, Some(&bd));
        assert_eq!(index.assets[0].status, ItemStatus::Done);
        assert_eq!(index.counts(Stage::Assets).ready, 1);
    }
}
