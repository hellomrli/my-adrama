//! Pipeline stages and their persisted review state (`state.json`).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The four gated production stages, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Parse,
    Assets,
    Storyboard,
    Video,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Parse, Stage::Assets, Stage::Storyboard, Stage::Video];

    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Parse => "parse",
            Stage::Assets => "assets",
            Stage::Storyboard => "storyboard",
            Stage::Video => "video",
        }
    }

    /// Display label. Defined once here so CLI and GUI never drift apart.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Parse => "解析",
            Stage::Assets => "资产",
            Stage::Storyboard => "分镜",
            Stage::Video => "视频",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Stage::Parse => "LLM 将剧本拆成角色 / 场景 / 镜头",
            Stage::Assets => "角色定妆照、服装、道具、场景参考图",
            Stage::Storyboard => "逐镜头生成画面，复用资产保持一致性",
            Stage::Video => "分镜图生视频片段",
        }
    }

    pub fn prev(self) -> Option<Stage> {
        match self {
            Stage::Parse => None,
            Stage::Assets => Some(Stage::Parse),
            Stage::Storyboard => Some(Stage::Assets),
            Stage::Video => Some(Stage::Storyboard),
        }
    }

    pub fn next(self) -> Option<Stage> {
        match self {
            Stage::Parse => Some(Stage::Assets),
            Stage::Assets => Some(Stage::Storyboard),
            Stage::Storyboard => Some(Stage::Video),
            Stage::Video => None,
        }
    }

    /// 1-based position, used for step badges.
    pub fn ordinal(self) -> usize {
        match self {
            Stage::Parse => 1,
            Stage::Assets => 2,
            Stage::Storyboard => 3,
            Stage::Video => 4,
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Stage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "parse" | "解析" => Ok(Stage::Parse),
            "assets" | "asset" | "资产" => Ok(Stage::Assets),
            "storyboard" | "分镜" => Ok(Stage::Storyboard),
            "video" | "视频" => Ok(Stage::Video),
            other => bail!("未知阶段：{other}（可选 parse|assets|storyboard|video）"),
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

impl StageStatus {
    pub fn label(self) -> &'static str {
        match self {
            StageStatus::Pending => "待处理",
            StageStatus::InProgress => "进行中",
            StageStatus::Done => "已完成",
            StageStatus::Approved => "已审核",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            StageStatus::Pending => "○",
            StageStatus::InProgress => "◐",
            StageStatus::Done => "●",
            StageStatus::Approved => "★",
        }
    }

    pub fn is_approved(self) -> bool {
        matches!(self, StageStatus::Approved)
    }
}

/// Review state of every stage — the gate that stops an expensive stage from
/// running before a human has looked at the previous one's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectState {
    #[serde(default)]
    pub parse: StageStatus,
    #[serde(default)]
    pub assets: StageStatus,
    #[serde(default)]
    pub storyboard: StageStatus,
    #[serde(default)]
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

    /// First stage that is not yet approved — i.e. where the user should work.
    pub fn current_stage(&self) -> Stage {
        Stage::ALL
            .into_iter()
            .find(|s| !self.get(*s).is_approved())
            .unwrap_or(Stage::Video)
    }
}

/// Status of one generated item (a character sheet, a storyboard frame, a clip).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    #[default]
    Pending,
    Generating,
    Done,
    Failed,
    Approved,
}

impl ItemStatus {
    pub fn label(self) -> &'static str {
        match self {
            ItemStatus::Pending => "待生成",
            ItemStatus::Generating => "生成中",
            ItemStatus::Done => "已生成",
            ItemStatus::Failed => "失败",
            ItemStatus::Approved => "已审核",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_stage_tracks_approvals() {
        let mut st = ProjectState::default();
        assert_eq!(st.current_stage(), Stage::Parse);
        st.set(Stage::Parse, StageStatus::Approved);
        assert_eq!(st.current_stage(), Stage::Assets);
        st.set(Stage::Assets, StageStatus::Done);
        assert_eq!(st.current_stage(), Stage::Assets);
    }

    #[test]
    fn stage_parses_from_cli_and_labels() {
        assert_eq!("Storyboard".parse::<Stage>().unwrap(), Stage::Storyboard);
        assert!("nope".parse::<Stage>().is_err());
        assert_eq!(Stage::Video.prev(), Some(Stage::Storyboard));
        assert_eq!(Stage::Video.next(), None);
    }
}
