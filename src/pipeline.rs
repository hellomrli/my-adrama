use anyhow::{bail, Result};

use crate::project::{Project, Stage, StageStatus};

fn stage_zh(stage: Stage) -> &'static str {
    match stage {
        Stage::Parse => "解析",
        Stage::Assets => "资产",
        Stage::Storyboard => "分镜",
        Stage::Video => "视频",
    }
}

fn status_zh(status: StageStatus) -> &'static str {
    match status {
        StageStatus::Pending => "待处理",
        StageStatus::InProgress => "进行中",
        StageStatus::Done => "已完成",
        StageStatus::Approved => "已审核",
    }
}

/// Stage may run if previous is approved (or it is the first stage).
pub fn require_stage(proj: &Project, stage: Stage) -> Result<()> {
    if let Some(prev) = stage.prev() {
        require_approved(proj, prev)?;
    }
    Ok(())
}

pub fn require_approved(proj: &Project, stage: Stage) -> Result<()> {
    let status = proj.state.get(stage);
    if status != StageStatus::Approved {
        bail!(
            "阶段「{}」尚未审核通过（当前：{}）。请检查产物后执行：adrama approve {} \
             或在界面点击「审核通过」。",
            stage_zh(stage),
            status_zh(status),
            stage
        );
    }
    Ok(())
}

pub fn approve(proj: &mut Project, stage: Stage) -> Result<()> {
    let status = proj.state.get(stage);
    if status == StageStatus::Pending && !stage_has_output(proj, stage) {
        bail!(
            "阶段「{}」尚无产物（状态：待处理）。请先运行该阶段。",
            stage_zh(stage)
        );
    }
    proj.state.set(stage, StageStatus::Approved);
    proj.save_state()?;
    Ok(())
}

fn stage_has_output(proj: &Project, stage: Stage) -> bool {
    match stage {
        Stage::Parse => proj.parsed_path().exists(),
        Stage::Assets => proj.assets_dir().join("characters").exists(),
        Stage::Storyboard => proj.storyboard_dir().exists(),
        Stage::Video => proj.video_dir().exists(),
    }
}

pub fn mark(proj: &mut Project, stage: Stage, status: StageStatus) -> Result<()> {
    proj.state.set(stage, status);
    proj.save_state()
}
