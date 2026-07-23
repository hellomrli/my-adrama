//! ComfyUI-style node graph for the drama pipeline.
//! Nodes are visual; edges are fixed pipeline links. Click a node to select / run.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::project::{Project, Stage, StageStatus};

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
            WfNodeKind::Parse => "LLM → breakdown.json",
            WfNodeKind::Assets => "角色 · 服化道 · 场景",
            WfNodeKind::Storyboard => "镜头参考图",
            WfNodeKind::Video => "图生视频 (Veo/Grok)",
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
            WfNodeKind::Script => Color32::from_rgb(70, 110, 160),
            WfNodeKind::Parse => Color32::from_rgb(90, 140, 90),
            WfNodeKind::Assets => Color32::from_rgb(160, 120, 60),
            WfNodeKind::Storyboard => Color32::from_rgb(130, 90, 160),
            WfNodeKind::Video => Color32::from_rgb(160, 70, 90),
            WfNodeKind::Export => Color32::from_rgb(80, 100, 120),
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
    dragging_bg: bool,
    drag_node: Option<usize>,
    last_pointer: Pos2,
}

impl Default for WorkflowCanvas {
    fn default() -> Self {
        Self {
            nodes: default_nodes(),
            pan: Vec2::new(40.0, 80.0),
            zoom: 1.0,
            selected: Some(WfNodeKind::Script),
            dragging_bg: false,
            drag_node: None,
            last_pointer: Pos2::ZERO,
        }
    }
}

fn default_nodes() -> Vec<WfNode> {
    let w = 200.0;
    let h = 110.0;
    let gap_x = 260.0;
    let y = 40.0;
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
        pos: Pos2::new(i as f32 * gap_x, y + if i % 2 == 1 { 40.0 } else { 0.0 }),
        size: Vec2::new(w, h),
    })
    .collect()
}

impl WorkflowCanvas {
    pub fn reset_layout(&mut self) {
        self.nodes = default_nodes();
        self.pan = Vec2::new(40.0, 80.0);
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
        let (response, painter) =
            ui.allocate_painter(available, Sense::click_and_drag());

        let rect = response.rect;
        // background
        painter.rect_filled(rect, 0.0, Color32::from_rgb(28, 28, 32));
        draw_grid(&painter, rect, self.pan, self.zoom);

        // zoom with scroll
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let factor = (1.0 + scroll * 0.001).clamp(0.85, 1.15);
                self.zoom = (self.zoom * factor).clamp(0.5, 1.8);
            }
        }

        // pan
        if response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary)
                && self.drag_node.is_none()
                && ui.input(|i| i.modifiers.command || i.modifiers.alt))
        {
            self.pan += response.drag_delta();
            self.dragging_bg = true;
        } else if response.dragged_by(egui::PointerButton::Primary) && self.drag_node.is_none() {
            // allow empty-space pan with primary if not on node
            if let Some(pos) = response.interact_pointer_pos() {
                if !self.hit_node(pos, rect).is_some() {
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
            let c0 = Pos2::new(p0.x + 40.0 * self.zoom, p0.y);
            let c1 = Pos2::new(p1.x - 40.0 * self.zoom, p1.y);
            let stroke = Stroke::new(2.5 * self.zoom, Color32::from_rgb(90, 95, 110));
            painter.add(egui::Shape::CubicBezier(egui::epaint::CubicBezierShape {
                points: [p0, c0, c1, p1],
                closed: false,
                fill: Color32::TRANSPARENT,
                stroke: stroke.into(),
            }));
            // arrow head
            let dir = (p1 - c1).normalized();
            let orth = Vec2::new(-dir.y, dir.x) * 6.0 * self.zoom;
            let tip = p1;
            let base = tip - dir * 12.0 * self.zoom;
            painter.add(egui::Shape::convex_polygon(
                vec![tip, base + orth, base - orth],
                Color32::from_rgb(90, 95, 110),
                Stroke::NONE,
            ));
        }

        // pointer interactions for nodes
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
            self.dragging_bg = false;
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

        // draw nodes
        for node in &self.nodes {
            let r = self.screen_rect(rect, node);
            let selected = self.selected == Some(node.kind);
            let status = node
                .kind
                .stage()
                .and_then(|s| project.map(|p| p.state.get(s)));
            let running = busy_kind == Some(node.kind);

            let mut fill = Color32::from_rgb(40, 42, 48);
            let border = if selected {
                Color32::from_rgb(100, 180, 255)
            } else if running {
                Color32::from_rgb(80, 200, 255)
            } else {
                Color32::from_rgb(70, 72, 80)
            };
            if let Some(st) = status {
                fill = match st {
                    StageStatus::Pending => Color32::from_rgb(40, 42, 48),
                    StageStatus::InProgress => Color32::from_rgb(35, 50, 70),
                    StageStatus::Done => Color32::from_rgb(35, 55, 40),
                    StageStatus::Approved => Color32::from_rgb(50, 48, 30),
                };
            }

            let rounding = 8.0_f32;
            painter.rect_filled(r, rounding, fill);
            painter.rect_stroke(
                r,
                rounding,
                Stroke::new(if selected { 2.5_f32 } else { 1.5_f32 }, border),
                egui::StrokeKind::Outside,
            );

            // title bar
            let title_h = 28.0 * self.zoom;
            let title_rect = Rect::from_min_size(r.min, Vec2::new(r.width(), title_h));
            painter.rect_filled(title_rect.intersect(r), rounding, node.kind.color());
            painter.text(
                title_rect.left_center() + Vec2::new(10.0 * self.zoom, 0.0),
                Align2::LEFT_CENTER,
                node.kind.title(),
                FontId::proportional(14.0 * self.zoom),
                Color32::WHITE,
            );

            // status badge
            if let Some(st) = status {
                let badge = status_zh(st);
                painter.text(
                    title_rect.right_center() - Vec2::new(8.0 * self.zoom, 0.0),
                    Align2::RIGHT_CENTER,
                    badge,
                    FontId::proportional(11.0 * self.zoom),
                    Color32::from_rgb(240, 240, 240),
                );
            } else if node.kind == WfNodeKind::Script {
                let has = project
                    .map(|p| p.find_script().is_ok())
                    .unwrap_or(false);
                painter.text(
                    title_rect.right_center() - Vec2::new(8.0 * self.zoom, 0.0),
                    Align2::RIGHT_CENTER,
                    if has { "已导入" } else { "待导入" },
                    FontId::proportional(11.0 * self.zoom),
                    Color32::from_rgb(240, 240, 240),
                );
            }

            painter.text(
                r.min + Vec2::new(12.0 * self.zoom, title_h + 12.0 * self.zoom),
                Align2::LEFT_TOP,
                node.kind.subtitle(),
                FontId::proportional(12.0 * self.zoom),
                Color32::from_rgb(180, 185, 195),
            );

            // sockets
            let in_pos = Pos2::new(r.left(), r.center().y);
            let out_pos = Pos2::new(r.right(), r.center().y);
            if node.kind != WfNodeKind::Script {
                painter.circle_filled(in_pos, 5.0 * self.zoom, Color32::from_rgb(120, 160, 220));
            }
            if node.kind != WfNodeKind::Export {
                painter.circle_filled(out_pos, 5.0 * self.zoom, Color32::from_rgb(120, 160, 220));
            }

            if running {
                painter.text(
                    r.center_bottom() - Vec2::new(0.0, 10.0 * self.zoom),
                    Align2::CENTER_BOTTOM,
                    "运行中…",
                    FontId::proportional(11.0 * self.zoom),
                    Color32::LIGHT_BLUE,
                );
            }
        }

        // overlay toolbar on canvas
        egui::Area::new(egui::Id::new("wf_toolbar"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top() + Vec2::new(12.0, 12.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(30, 30, 36, 230))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("工作流画布").strong());
                            ui.separator();
                            if ui.button("重置布局").clicked() {
                                self.reset_layout();
                            }
                            if ui.button("适应视图").clicked() {
                                self.pan = Vec2::new(40.0, 80.0);
                                self.zoom = 1.0;
                            }
                            ui.label(
                                egui::RichText::new(format!("缩放 {:.0}%", self.zoom * 100.0))
                                    .small()
                                    .weak(),
                            );
                        });
                        ui.label(
                            egui::RichText::new(
                                "拖拽节点 · 滚轮缩放 · Alt/中键拖动画布 · 双击节点打开详情",
                            )
                            .small()
                            .weak(),
                        );
                    });
            });

        // inspector for selected node
        if let Some(kind) = self.selected {
            egui::Area::new(egui::Id::new("wf_inspector"))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.right_top() + Vec2::new(-320.0, 12.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(Color32::from_rgba_unmultiplied(30, 30, 36, 240))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_width(290.0);
                            ui.heading(kind.title());
                            ui.label(egui::RichText::new(kind.subtitle()).weak());
                            ui.separator();
                            if let Some(stage) = kind.stage() {
                                if let Some(p) = project {
                                    let st = p.state.get(stage);
                                    ui.label(format!("阶段状态：{}", status_zh(st)));
                                }
                            }
                            ui.add_space(6.0);
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
                                ) {
                                    if ui
                                        .add_enabled(!busy && project.is_some(), egui::Button::new("▶ 运行"))
                                        .clicked()
                                    {
                                        action = WfAction::Run(kind);
                                    }
                                }
                                if let Some(stage) = kind.stage() {
                                    if ui
                                        .add_enabled(
                                            !busy && project.is_some(),
                                            egui::Button::new("★ 审核通过"),
                                        )
                                        .clicked()
                                    {
                                        action = WfAction::Approve(stage);
                                    }
                                }
                            });
                            ui.add_space(4.0);
                            match kind {
                                WfNodeKind::Script => {
                                    ui.label("导入剧本 Markdown，作为整条流水线的输入。");
                                }
                                WfNodeKind::Parse => {
                                    ui.label("调用对话模型解析角色 / 场景 / 镜头结构。");
                                    if let Some(p) = project {
                                        ui.monospace(format!(
                                            "Chat: {} @ {}",
                                            p.config.openai_chat_model,
                                            short_url(&p.config.openai_base_url)
                                        ));
                                    }
                                }
                                WfNodeKind::Assets | WfNodeKind::Storyboard => {
                                    ui.label("调用 Image2 / Grok 等图像模型生成参考图。");
                                    if let Some(p) = project {
                                        ui.monospace(format!(
                                            "Image: {} @ {}",
                                            p.config.openai_image_model,
                                            short_url(&p.config.openai_base_url)
                                        ));
                                    }
                                }
                                WfNodeKind::Video => {
                                    ui.label("调用 Veo / Omni / Grok 视频等图生视频接口。");
                                    if let Some(p) = project {
                                        ui.monospace(format!(
                                            "Video: {} @ {}",
                                            p.config.google_video_model,
                                            short_url(&p.config.google_base_url)
                                        ));
                                    }
                                }
                                WfNodeKind::Export => {
                                    ui.label("使用系统 ffmpeg 按镜头顺序拼接 mp4。");
                                }
                            }
                        });
                });
        }

        // request repaint while interacting
        if self.dragging_bg || self.drag_node.is_some() {
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

fn draw_grid(painter: &egui::Painter, rect: Rect, pan: Vec2, zoom: f32) {
    let step = 24.0 * zoom;
    if step < 8.0 {
        return;
    }
    let col = Color32::from_rgb(38, 38, 44);
    let origin = rect.min + pan;
    let mut x = origin.x % step;
    if x < 0.0 {
        x += step;
    }
    while x < rect.width() {
        let px = rect.left() + x;
        painter.line_segment(
            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
            Stroke::new(1.0_f32, col),
        );
        x += step;
    }
    let mut y = origin.y % step;
    if y < 0.0 {
        y += step;
    }
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

fn short_url(u: &str) -> String {
    if u.len() <= 42 {
        u.to_string()
    } else {
        format!("{}…", &u[..40])
    }
}
