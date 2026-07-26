//! Screenplay editor — import a file or type directly into the project.

use eframe::egui::{self, RichText, Ui};

use super::ViewCtx;
use crate::model::Project;
use crate::ui::state::View;
use crate::ui::{theme, widgets};

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(ui, View::Script.title(), View::Script.subtitle());

    let Some(root) = cx.state.root() else {
        widgets::empty_state(ui, "尚未打开项目", "在「概览」中打开或新建一个项目。");
        return;
    };

    let script_path = cx
        .state
        .snapshot
        .as_ref()
        .and_then(|s| s.script_path.clone());
    let dirty = cx.state.script_dirty;
    let chars = cx.state.script_text.chars().count();

    let mut do_import = false;
    let mut do_save = false;
    let mut do_revert = false;
    let mut do_format = false;

    ui.horizontal(|ui| {
        if widgets::button(ui, "导入文件…", !cx.state.is_busy()) {
            do_import = true;
        }
        if widgets::primary_button(ui, if dirty { "保存 *" } else { "保存" }, dirty) {
            do_save = true;
        }
        if widgets::button(ui, "放弃修改", dirty) {
            do_revert = true;
        }
        ui.separator();
        if widgets::button(ui, "AI 格式化剧本", !cx.state.is_busy() && script_path.is_some() && !dirty)
        {
            do_format = true;
        }
        if dirty {
            widgets::hint(ui, "（先保存才能格式化）");
        }
        ui.separator();
        match &script_path {
            Some(path) => widgets::path_label(ui, path),
            None => widgets::hint(ui, "尚未保存到文件（保存后写入 script/script.md）"),
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::hint(ui, &format!("{chars} 字"));
        });
    });

    ui.add_space(theme::SPACE_SM);

    theme::card().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let response = ui.add(
                    egui::TextEdit::multiline(&mut cx.state.script_text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(26)
                        .font(egui::TextStyle::Monospace),
                );
                if response.changed() {
                    cx.state.script_dirty = true;
                }
            });
    });

    ui.add_space(theme::SPACE_SM);
    ui.label(
        RichText::new("提示：写清场次、人物、动作与台词；拆解阶段会据此生成角色表与镜头表。")
            .small()
            .color(theme::TEXT_DIM),
    );

    // --- actions ---
    if do_import {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("剧本", &["md", "txt", "fountain"])
            .pick_file()
        {
            match Project::open(&root).and_then(|p| p.import_script(&path)) {
                Ok(dest) => {
                    cx.state.script_dirty = false;
                    cx.state.note(format!("已导入 {}", dest.display()));
                    cx.state.refresh(cx.runtime);
                }
                Err(err) => cx.state.fail(format!("导入失败：{err:#}")),
            }
        }
    }

    if do_save {
        let text = cx.state.script_text.clone();
        match Project::open(&root).and_then(|p| p.write_script(&text)) {
            Ok(path) => {
                cx.state.script_dirty = false;
                cx.state.note(format!("剧本已保存 → {}", path.display()));
                cx.state.refresh(cx.runtime);
            }
            Err(err) => cx.state.fail(format!("保存失败：{err:#}")),
        }
    }

    if do_revert {
        if let Some(snapshot) = &cx.state.snapshot {
            cx.state.script_text = snapshot.script_text.clone();
        }
        cx.state.script_dirty = false;
    }

    if do_format {
        cx.state.push_console(
            crate::engine::events::Level::Info,
            "格式化：整理成标准影视剧本模板（场次/内外景/△动作/台词/OS），原稿自动备份为 .bak",
        );
        cx.state.submit(cx.runtime, crate::engine::Job::FormatScript);
    }
}
