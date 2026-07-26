//! Desktop front-end (egui/eframe).
//!
//! Layers: `runtime` owns the worker thread, `state` owns mutable UI state,
//! `views` draw, `widgets`/`theme` provide the vocabulary. Nothing here reaches
//! into HTTP or blocks the frame loop.

pub mod app;
mod fonts;
mod runtime;
mod state;
mod theme;
mod thumbs;
mod views;
mod widgets;

use anyhow::Result;
use std::path::PathBuf;

pub fn run(initial_project: PathBuf) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 940.0])
            .with_min_inner_size([1080.0, 700.0])
            .with_title("adrama — AI 短剧工作流"),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "adrama",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::AdramaApp::new(cc, initial_project)))
        }),
    )
    .map_err(|err| anyhow::anyhow!("图形界面启动失败：{err}"))
}
