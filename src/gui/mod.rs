mod app;
mod fonts;
mod theme;
mod worker;
mod workflow;

use anyhow::Result;
use std::path::PathBuf;

pub fn run(initial_project: PathBuf) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 700.0])
            .with_title("my-adrama — AI 短剧工作流"),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "my-adrama",
        options,
        Box::new(move |cc| {
            fonts::install_cjk_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::AdramaApp::new(cc, initial_project)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("图形界面启动失败: {e}"))
}
