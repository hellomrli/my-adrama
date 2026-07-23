use anyhow::Result;
use std::fs;

use crate::project::{Project, Stage, StageStatus};

pub fn run(proj: &Project) -> Result<()> {
    println!("项目: {} ({})", proj.config.name, proj.root.display());
    println!("风格: {}", proj.config.style);
    println!("画幅: {}", proj.config.aspect);
    println!(
        "路由: 对话={:?}  图像={:?}  视频={:?}",
        proj.config.chat_provider, proj.config.image_provider, proj.config.video_provider
    );
    println!();
    println!("阶段:");
    for stage in Stage::all() {
        let status = proj.state.get(*stage);
        let mark = match status {
            StageStatus::Pending => " ",
            StageStatus::InProgress => "…",
            StageStatus::Done => "✓",
            StageStatus::Approved => "★",
        };
        println!(
            "  [{mark}] {:<8} {}",
            stage_zh(*stage),
            status_zh(status)
        );
    }

    if proj.parsed_path().exists() {
        if let Ok(bd) = proj.load_breakdown() {
            println!();
            println!(
                "结构化: {} 角色, {} 场景地, {} 场, {} 镜头",
                bd.characters.len(),
                bd.locations.len(),
                bd.scenes.len(),
                bd.shots.len()
            );
        }
    }

    let sb_count = count_files(&proj.storyboard_dir(), "png");
    let vid_count = count_files(&proj.video_dir(), "mp4");
    if sb_count > 0 || vid_count > 0 {
        println!("产物: {sb_count} 张分镜图, {vid_count} 个视频片段");
    }

    let final_path = proj.video_dir().join("final.mp4");
    if final_path.exists() {
        println!("成片: {}", final_path.display());
    }

    Ok(())
}

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

fn count_files(dir: &std::path::Path, ext: &str) -> usize {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case(ext))
                .unwrap_or(false)
        })
        .count()
}
