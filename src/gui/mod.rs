mod app;
mod fonts;
mod worker;
mod workflow;

use anyhow::Result;
use std::path::PathBuf;

pub fn run(initial_project: PathBuf) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1024.0, 640.0])
            .with_title("adrama — AI 短剧工作流"),
        ..Default::default()
    };

    eframe::run_native(
        "adrama",
        options,
        Box::new(move |cc| {
            fonts::install_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(app::AdramaApp::new(cc, initial_project)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("图形界面启动失败: {e}"))
}
