//! Stage gating: the cheap safety rail that stops an expensive stage from
//! running on unreviewed input.

use anyhow::{bail, Result};

use crate::model::{Project, ProjectIndex, Stage, StageStatus};

/// A stage may run once its predecessor is approved.
pub fn require_ready(project: &Project, stage: Stage) -> Result<()> {
    match stage.prev() {
        Some(prev) => require_approved(project, prev),
        None => Ok(()),
    }
}

pub fn require_approved(project: &Project, stage: Stage) -> Result<()> {
    let status = project.state.get(stage);
    if !status.is_approved() {
        bail!(
            "阶段「{}」尚未审核通过（当前：{}）。请检查产物后点击「审核通过」，或执行 `adrama approve {stage}`。",
            stage.label(),
            status.label()
        );
    }
    Ok(())
}

/// Mark a stage approved. Refuses when the stage has produced nothing, so a
/// stray click cannot unlock video generation on an empty storyboard.
pub fn approve(project: &mut Project, stage: Stage) -> Result<()> {
    let evidence = output_summary(project, stage);
    if !evidence.has_output {
        bail!(
            "阶段「{}」尚无产物（{}），请先运行该阶段。",
            stage.label(),
            evidence.detail
        );
    }
    project.state.set(stage, StageStatus::Approved);
    project.save_state()?;
    Ok(())
}

/// Undo an approval so the stage can be reworked.
pub fn reset(project: &mut Project, stage: Stage) -> Result<()> {
    let status = if output_summary(project, stage).has_output {
        StageStatus::Done
    } else {
        StageStatus::Pending
    };
    project.state.set(stage, status);
    // Downstream approvals are no longer trustworthy once this stage reopens.
    let mut cursor = stage.next();
    while let Some(next) = cursor {
        if project.state.get(next).is_approved() {
            project.state.set(next, StageStatus::Done);
        }
        cursor = next.next();
    }
    project.save_state()?;
    Ok(())
}

pub fn mark(project: &mut Project, stage: Stage, status: StageStatus) -> Result<()> {
    project.state.set(stage, status);
    project.save_state()
}

pub struct OutputSummary {
    pub has_output: bool,
    pub detail: String,
}

/// Does this stage have real artifacts? The old check only asked whether the
/// output *directory* existed — which `Project::create` guarantees, so every
/// stage looked complete from the start.
pub fn output_summary(project: &Project, stage: Stage) -> OutputSummary {
    match stage {
        Stage::Parse => match project.load_breakdown() {
            Ok(bd) if !bd.shots.is_empty() => OutputSummary {
                has_output: true,
                detail: format!("{} 个镜头", bd.shots.len()),
            },
            Ok(_) => OutputSummary {
                has_output: false,
                detail: "breakdown.json 中没有镜头".into(),
            },
            Err(_) => OutputSummary {
                has_output: false,
                detail: "缺少 parsed/breakdown.json".into(),
            },
        },
        other => {
            let breakdown = project.load_breakdown().ok();
            let index = ProjectIndex::build(project, breakdown.as_ref());
            let counts = index.counts(other);
            OutputSummary {
                has_output: counts.ready > 0,
                detail: counts.summary(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AspectRatio, Breakdown, Shot};
    use std::fs;

    fn project() -> (tempfile::TempDir, Project) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        let proj = Project::create(&root, "p", "s", AspectRatio::Landscape).unwrap();
        (tmp, proj)
    }

    fn shot(id: &str) -> Shot {
        Shot {
            id: id.into(),
            scene_id: "sc1".into(),
            number: 1,
            framing: "wide".into(),
            camera: "static".into(),
            visual: "x".into(),
            dialogue: String::new(),
            sfx: String::new(),
            duration_secs: 5,
            character_ids: vec![],
            prop_ids: vec![],
            location_id: None,
        }
    }

    #[test]
    fn approving_an_empty_stage_is_refused() {
        let (_tmp, mut proj) = project();
        // storyboard/ exists because create() made it — that must not count.
        assert!(proj.storyboard_dir().is_dir());
        let err = approve(&mut proj, Stage::Storyboard).unwrap_err();
        assert!(err.to_string().contains("尚无产物"), "{err}");
    }

    #[test]
    fn approve_unlocks_the_next_stage() {
        let (_tmp, mut proj) = project();
        proj.save_breakdown(&Breakdown {
            shots: vec![shot("shot_1")],
            ..Default::default()
        })
        .unwrap();

        assert!(require_ready(&proj, Stage::Assets).is_err());
        approve(&mut proj, Stage::Parse).unwrap();
        assert!(require_ready(&proj, Stage::Assets).is_ok());
        assert_eq!(proj.state.parse, StageStatus::Approved);
    }

    #[test]
    fn reset_revokes_downstream_approvals() {
        let (_tmp, mut proj) = project();
        proj.save_breakdown(&Breakdown {
            shots: vec![shot("shot_1")],
            ..Default::default()
        })
        .unwrap();
        fs::write(proj.storyboard_image("shot_1"), b"png").unwrap();

        approve(&mut proj, Stage::Parse).unwrap();
        approve(&mut proj, Stage::Storyboard).unwrap();
        reset(&mut proj, Stage::Parse).unwrap();

        assert_eq!(proj.state.parse, StageStatus::Done);
        assert_eq!(proj.state.storyboard, StageStatus::Done);
        assert!(require_ready(&proj, Stage::Assets).is_err());
    }
}
