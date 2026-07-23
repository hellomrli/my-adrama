//! Pipeline node canvas — visual stage graph.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use super::theme;
use crate::project::{EndpointMode, Project, Stage, StageStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WfNodeKind {
    Script,
    Parse,
    Assets,
    Storyboard,
    Video,
    Export,
}

impl WfNodeKind {
    pub fn title(self) -> &'static str {
        match self {
            WfNodeKind::Script => "剧本",
            WfNodeKind::Parse => "解析",
            WfNodeKind::Assets => "资产",
            WfNodeKind::Storyboard => "分镜",
            WfNodeKind::Video => "视频",
            WfNodeKind::Export => "成片",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            WfNodeKind::Script => "导入 / 编辑原文",
            WfNodeKind::Parse => "LLM → 结构化 JSON",
            WfNodeKind::Assets => "角色 · 服化道 · 场景",
            WfNodeKind::Storyboard => "镜头参考图",
            WfNodeKind::Video => "图生视频",
            WfNodeKind::Export => "ffmpeg 拼接",
        }
    }

    pub fn stage(self) -> Option<Stage> {
        match self {
            WfNodeKind::Parse => Some(Stage::Parse),
            WfNodeKind::Assets => Some(Stage::Assets),
            WfNodeKind::Storyboard => Some(Stage::Storyboard),
            WfNodeKind::Video => Some(Stage::Video),
            _ => None,
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            WfNodeKind::Script => theme::STAGE_SCRIPT,
            WfNodeKind::Parse => theme::STAGE_PARSE,
            WfNodeKind::Assets => theme::STAGE_ASSETS,
            WfNodeKind::Storyboard => theme::STAGE_STORY,
            WfNodeKind::Video => theme::STAGE_VIDEO,
            WfNodeKind::Export => theme::STAGE_EXPORT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WfNode {
    pub kind: WfNodeKind,
    pub pos: Pos2,
    pub size: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WfAction {
    None,
    Select(WfNodeKind),
    OpenTab(WfNodeKind),
    Run(WfNodeKind),
    Approve(Stage),
}

pub struct WorkflowCanvas {
    pub nodes: Vec<WfNode>,
    pub pan: Vec2,
    pub zoom: f32,
    pub selected: Option<WfNodeKind>,
    drag_node: Option<usize>,
    last_pointer: Pos2,
}

impl Default for WorkflowCanvas {
    fn default() -> Self {
        Self {
            nodes: default_nodes(),
            pan: Vec2::new(48.0, 100.0),
            zoom: 1.0,
            selected: Some(WfNodeKind::Script),
            drag_node: None,
            last_pointer: Pos2::ZERO,
        }
    }
}

fn default_nodes() -> Vec<WfNode> {
    let w = 196.0;
    let h = 118.0;
    let gap = 248.0;
    [
        WfNodeKind::Script,
        WfNodeKind::Parse,
        WfNodeKind::Assets,
        WfNodeKind::Storyboard,
        WfNodeKind::Video,
        WfNodeKind::Export,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| WfNode {
        kind,
        pos: Pos2::new(
            i as f32 * gap,
            if i % 2 == 1 { 48.0 } else { 0.0 },
        ),
        size: Vec2::new(w, h),
    })
    .collect()
}

impl WorkflowCanvas {
    pub fn reset_layout(&mut self) {
        self.nodes = default_nodes();
        self.pan = Vec2::new(48.0, 100.0);
        self.zoom = 1.0;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        project: Option<&Project>,
        busy: bool,
        busy_kind: Option<WfNodeKind>,
    ) -> WfAction {
        let mut action = WfAction::None;
        let available = ui.available_size();
        let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
        let rect = response.rect;

        painter.rect_filled(rect, 0.0, theme::BG);
        draw_grid(&painter, rect, self.pan, self.zoom);

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let factor = (1.0 + scroll * 0.001).clamp(0.85, 1.15);
                self.zoom = (self.zoom * factor).clamp(0.55, 1.75);
            }
        }

        if response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary)
                && self.drag_node.is_none()
                && ui.input(|i| i.modifiers.command || i.modifiers.alt))
        {
            self.pan += response.drag_delta();
        } else if response.dragged_by(egui::PointerButton::Primary) && self.drag_node.is_none() {
            if let Some(pos) = response.interact_pointer_pos() {
                if self.hit_node(pos, rect).is_none() {
                    self.pan += response.drag_delta();
                }
            }
        }

        // edges
        for i in 0..self.nodes.len().saturating_sub(1) {
            let a = self.screen_rect(rect, &self.nodes[i]);
            let b = self.screen_rect(rect, &self.nodes[i + 1]);
            let p0 = Pos2::new(a.right(), a.center().y);
            let p1 = Pos2::new(b.left(), b.center().y);
            let c0 = Pos2::new(p0.x + 48.0 * self.zoom, p0.y);
            let c1 = Pos2::new(p1.x - 48.0 * self.zoom, p1.y);
            let stroke = Stroke::new(2.5 * self.zoom, Color32::from_rgb(70, 78, 100));
            painter.add(egui::Shape::CubicBezier(egui::epaint::CubicBezierShape {
                points: [p0, c0, c1, p1],
                closed: false,
                fill: Color32::TRANSPARENT,
                stroke: stroke.into(),
            }));
            let dir = (p1 - c1).normalized();
            let orth = Vec2::new(-dir.y, dir.x) * 6.0 * self.zoom;
            let tip = p1;
            let base = tip - dir * 12.0 * self.zoom;
            painter.add(egui::Shape::convex_polygon(
                vec![tip, base + orth, base - orth],
                Color32::from_rgb(70, 78, 100),
                Stroke::NONE,
            ));
        }

        let pointer = response.interact_pointer_pos();
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pos) = pointer {
                self.drag_node = self.hit_node(pos, rect);
                self.last_pointer = pos;
            }
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(idx), Some(pos)) = (self.drag_node, pointer) {
                let delta = (pos - self.last_pointer) / self.zoom;
                self.nodes[idx].pos += delta;
                self.last_pointer = pos;
            }
        }
        if response.drag_stopped() {
            self.drag_node = None;
        }

        if response.clicked() {
            if let Some(pos) = pointer {
                if let Some(idx) = self.hit_node(pos, rect) {
                    let kind = self.nodes[idx].kind;
                    self.selected = Some(kind);
                    action = WfAction::Select(kind);
                }
            }
        }
        if response.double_clicked() {
            if let Some(pos) = pointer {
                if let Some(idx) = self.hit_node(pos, rect) {
                    action = WfAction::OpenTab(self.nodes[idx].kind);
                }
            }
        }

        for node in &self.nodes {
            let r = self.screen_rect(rect, node);
            let selected = self.selected == Some(node.kind);
            let status = node
                .kind
                .stage()
                .and_then(|s| project.map(|p| p.state.get(s)));
            let running = busy_kind == Some(node.kind);

            let mut fill = theme::BG_PANEL;
            if let Some(st) = status {
                fill = match st {
                    StageStatus::Pending => theme::BG_PANEL,
                    StageStatus::InProgress => Color32::from_rgb(24, 36, 52),
                    StageStatus::Done => Color32::from_rgb(22, 40, 32),
                    StageStatus::Approved => Color32::from_rgb(42, 38, 24),
                };
            }
            let border = if selected {
                theme::ACCENT
            } else if running {
                theme::INFO
            } else {
                theme::BORDER
            };

            painter.rect_filled(r, 12.0, fill);
            painter.rect_stroke(
                r,
                12.0,
                Stroke::new(if selected { 2.2_f32 } else { 1.2_f32 }, border),
                egui::StrokeKind::Outside,
            );

            let title_h = 32.0 * self.zoom;
            let title_rect = Rect::from_min_size(r.min, Vec2::new(r.width(), title_h));
            painter.rect_filled(title_rect.intersect(r), 12.0, node.kind.color());
            painter.text(
                title_rect.left_center() + Vec2::new(12.0 * self.zoom, 0.0),
                Align2::LEFT_CENTER,
                node.kind.title(),
                FontId::proportional(15.0 * self.zoom),
                Color32::WHITE,
            );

            if let Some(st) = status {
                painter.text(
                    title_rect.right_center() - Vec2::new(10.0 * self.zoom, 0.0),
                    Align2::RIGHT_CENTER,
                    status_zh(st),
                    FontId::proportional(11.0 * self.zoom),
                    Color32::from_rgb(245, 245, 250),
                );
            } else if node.kind == WfNodeKind::Script {
                let has = project.map(|p| p.find_script().is_ok()).unwrap_or(false);
                painter.text(
                    title_rect.right_center() - Vec2::new(10.0 * self.zoom, 0.0),
                    Align2::RIGHT_CENTER,
                    if has { "已导入" } else { "待导入" },
                    FontId::proportional(11.0 * self.zoom),
                    Color32::from_rgb(245, 245, 250),
                );
            }

            painter.text(
                r.min + Vec2::new(12.0 * self.zoom, title_h + 14.0 * self.zoom),
                Align2::LEFT_TOP,
                node.kind.subtitle(),
                FontId::proportional(12.0 * self.zoom),
                theme::TEXT_MUTED,
            );

            let in_pos = Pos2::new(r.left(), r.center().y);
            let out_pos = Pos2::new(r.right(), r.center().y);
            if node.kind != WfNodeKind::Script {
                painter.circle_filled(in_pos, 5.5 * self.zoom, theme::ACCENT);
            }
            if node.kind != WfNodeKind::Export {
                painter.circle_filled(out_pos, 5.5 * self.zoom, theme::ACCENT);
            }
            if running {
                painter.text(
                    r.center_bottom() - Vec2::new(0.0, 12.0 * self.zoom),
                    Align2::CENTER_BOTTOM,
                    "运行中…",
                    FontId::proportional(11.0 * self.zoom),
                    theme::INFO,
                );
            }
        }

        // floating toolbar
        egui::Area::new(egui::Id::new("wf_toolbar"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top() + Vec2::new(14.0, 14.0))
            .show(ui.ctx(), |ui| {
                theme::section_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("工作流").strong().color(theme::TEXT));
                        ui.separator();
                        if ui.button("重置布局").clicked() {
                            self.reset_layout();
                        }
                        if ui.button("适应视图").clicked() {
                            self.pan = Vec2::new(48.0, 100.0);
                            self.zoom = 1.0;
                        }
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    ui.label(
                        egui::RichText::new("拖拽节点 · 滚轮缩放 · Alt/中键平移 · 双击打开")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
            });

        // inspector
        if let Some(kind) = self.selected {
            egui::Area::new(egui::Id::new("wf_inspector"))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.right_top() + Vec2::new(-340.0, 14.0))
                .show(ui.ctx(), |ui| {
                    theme::section_frame().show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter().circle_filled(rect.center(), 5.0, kind.color());
                            ui.heading(kind.title());
                        });
                        ui.label(egui::RichText::new(kind.subtitle()).color(theme::TEXT_MUTED));
                        ui.add_space(6.0);
                        ui.separator();
                        if let Some(stage) = kind.stage() {
                            if let Some(p) = project {
                                ui.label(format!("状态：{}", status_zh(p.state.get(stage))));
                            }
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("打开详情"))
                                .clicked()
                            {
                                action = WfAction::OpenTab(kind);
                            }
                            if matches!(
                                kind,
                                WfNodeKind::Parse
                                    | WfNodeKind::Assets
                                    | WfNodeKind::Storyboard
                                    | WfNodeKind::Video
                                    | WfNodeKind::Export
                                    | WfNodeKind::Script
                            ) {
                                if ui
                                    .add_enabled(
                                        !busy && (project.is_some() || kind == WfNodeKind::Script),
                                        egui::Button::new("▶ 运行"),
                                    )
                                    .clicked()
                                {
                                    action = WfAction::Run(kind);
                                }
                            }
                            if let Some(stage) = kind.stage() {
                                if ui
                                    .add_enabled(
                                        !busy && project.is_some(),
                                        egui::Button::new("★ 审核"),
                                    )
                                    .clicked()
                                {
                                    action = WfAction::Approve(stage);
                                }
                            }
                        });
                        if let Some(p) = project {
                            ui.add_space(8.0);
                            match kind {
                                WfNodeKind::Parse => {
                                    ui.monospace(format!(
                                        "Chat · {} · {}",
                                        p.config.chat_provider.label_zh(),
                                        mode_tag(match p.config.chat_provider {
                                            crate::project::ProviderKind::OpenAi => p.config.openai_mode,
                                            crate::project::ProviderKind::Google => p.config.google_mode,
                                            crate::project::ProviderKind::Xai => p.config.xai_mode,
                                        })
                                    ));
                                }
                                WfNodeKind::Assets | WfNodeKind::Storyboard => {
                                    ui.monospace(format!(
                                        "Image · {} · {}",
                                        p.config.image_provider.label_zh(),
                                        mode_tag(match p.config.image_provider {
                                            crate::project::ProviderKind::OpenAi => p.config.openai_mode,
                                            crate::project::ProviderKind::Google => p.config.google_mode,
                                            crate::project::ProviderKind::Xai => p.config.xai_mode,
                                        })
                                    ));
                                }
                                WfNodeKind::Video => {
                                    ui.monospace(format!(
                                        "Video · {} · {}",
                                        p.config.video_provider.label_zh(),
                                        mode_tag(match p.config.video_provider {
                                            crate::project::ProviderKind::OpenAi => p.config.openai_mode,
                                            crate::project::ProviderKind::Google => p.config.google_mode,
                                            crate::project::ProviderKind::Xai => p.config.xai_mode,
                                        })
                                    ));
                                }
                                _ => {}
                            }
                        }
                    });
                });
        }

        if self.drag_node.is_some() {
            ui.ctx().request_repaint();
        }
        action
    }

    fn screen_rect(&self, canvas: Rect, node: &WfNode) -> Rect {
        let min = canvas.min + self.pan + node.pos.to_vec2() * self.zoom;
        Rect::from_min_size(min, node.size * self.zoom)
    }

    fn hit_node(&self, pos: Pos2, canvas: Rect) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, n)| self.screen_rect(canvas, n).contains(pos))
            .map(|(i, _)| i)
    }
}

fn mode_tag(m: EndpointMode) -> &'static str {
    m.label_zh()
}

fn draw_grid(painter: &egui::Painter, rect: Rect, pan: Vec2, zoom: f32) {
    let step = 28.0 * zoom;
    if step < 8.0 {
        return;
    }
    let col = Color32::from_rgb(28, 30, 40);
    let origin = rect.min + pan;
    let mut x = origin.x.rem_euclid(step);
    while x < rect.width() {
        let px = rect.left() + x;
        painter.line_segment(
            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
            Stroke::new(1.0_f32, col),
        );
        x += step;
    }
    let mut y = origin.y.rem_euclid(step);
    while y < rect.height() {
        let py = rect.top() + y;
        painter.line_segment(
            [Pos2::new(rect.left(), py), Pos2::new(rect.right(), py)],
            Stroke::new(1.0_f32, col),
        );
        y += step;
    }
}

fn status_zh(st: StageStatus) -> &'static str {
    match st {
        StageStatus::Pending => "待处理",
        StageStatus::InProgress => "进行中",
        StageStatus::Done => "已完成",
        StageStatus::Approved => "已审核",
    }
}
