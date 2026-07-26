//! One module per screen. Views read from [`AppState`], draw, and queue
//! actions; they never call the engine directly except through `state.submit`.

pub mod breakdown;
pub mod dashboard;
pub mod flow;
pub mod script;
pub mod settings;
pub mod workbench;

use super::runtime::Runtime;
use super::state::AppState;
use super::thumbs::Thumbnails;

/// 任务运行时在页面顶部显示状态条。返回 true 表示本帧不必再画后面的内容。
pub fn running_banner(ui: &mut eframe::egui::Ui, cx: &mut ViewCtx<'_>) -> bool {
    let Some(busy) = &cx.state.busy else {
        return false;
    };
    let label = busy.label.clone();
    let elapsed = busy.started.elapsed();
    let detail = busy.progress.as_ref().map(|(_, _, d)| d.clone());
    let counts = busy.progress.as_ref().map(|(done, total, _)| (*done, *total));

    ui.add_space(super::theme::SPACE_SM);
    let cancel = super::widgets::running_strip(ui, &label, detail.as_deref(), elapsed, counts);
    if cancel {
        cx.runtime.cancel();
        cx.state.push_console(
            crate::engine::events::Level::Warn,
            "已请求取消，正在等待当前请求返回…",
        );
    }
    false
}

pub struct ViewCtx<'a> {
    pub state: &'a mut AppState,
    pub runtime: &'a Runtime,
    pub thumbs: &'a mut Thumbnails,
}
