//! 流程图：把当前项目**真实的**依赖关系画出来。
//!
//! 这不是一张装饰用的示意图——节点来自 breakdown，颜色是每一项的真实状态，
//! 连线是真实的依赖：哪些资产会被哪个分镜当参考图、哪个分镜生成哪个片段。
//! 点节点可以直接跳到对应条目。

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::ViewCtx;
use crate::model::{index::truncate, AssetKind, ItemStatus, Stage};
use crate::ui::state::View;
use crate::ui::{theme, widgets};

const COL_WIDTH: f32 = 226.0;
const NODE_WIDTH: f32 = 178.0;
const NODE_HEIGHT: f32 = 46.0;
const ROW_GAP: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Script,
    Parse,
    Asset(AssetKind),
    Storyboard,
    Video,
    Export,
}

impl NodeKind {
    fn column(self) -> usize {
        match self {
            NodeKind::Script => 0,
            NodeKind::Parse => 1,
            NodeKind::Asset(_) => 2,
            NodeKind::Storyboard => 3,
            NodeKind::Video => 4,
            NodeKind::Export => 5,
        }
    }

    fn stage(self) -> Option<Stage> {
        match self {
            NodeKind::Script => None,
            NodeKind::Parse => Some(Stage::Parse),
            NodeKind::Asset(_) => Some(Stage::Assets),
            NodeKind::Storyboard => Some(Stage::Storyboard),
            NodeKind::Video | NodeKind::Export => Some(Stage::Video),
        }
    }

    fn accent(self) -> Color32 {
        match self {
            NodeKind::Script => theme::TEXT_MUTED,
            NodeKind::Parse => theme::stage_color(Stage::Parse),
            NodeKind::Asset(_) => theme::stage_color(Stage::Assets),
            NodeKind::Storyboard => theme::stage_color(Stage::Storyboard),
            NodeKind::Video => theme::stage_color(Stage::Video),
            NodeKind::Export => theme::ACCENT,
        }
    }
}

struct Node {
    kind: NodeKind,
    /// 条目 id（资产 / 镜头），用于点击跳转。
    item_id: Option<String>,
    title: String,
    subtitle: String,
    status: Option<ItemStatus>,
    pos: Pos2,
}

struct Graph {
    nodes: Vec<Node>,
    edges: Vec<(usize, usize)>,
    size: Vec2,
}

pub fn show(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    widgets::page_header(ui, View::Flow.title(), View::Flow.subtitle());

    if cx.state.snapshot.is_none() {
        widgets::empty_state(ui, "尚未打开项目", "在「概览」中打开或新建一个项目。");
        return;
    }
    let has_breakdown = cx
        .state
        .snapshot
        .as_ref()
        .is_some_and(|s| s.breakdown.is_some());
    if !has_breakdown {
        widgets::empty_state(
            ui,
            "拆解之后这里会画出整条流程",
            "先在「拆解」页运行一次，程序会按角色、场景、镜头把后续要做的事全部展开。",
        );
        return;
    }

    toolbar(ui, cx);
    ui.add_space(theme::SPACE_SM);

    let graph = build_graph(cx);
    canvas(ui, cx, &graph);
}

fn toolbar(ui: &mut Ui, cx: &mut ViewCtx<'_>) {
    ui.horizontal_wrapped(|ui| {
        if widgets::button(ui, "适应视图", true) {
            cx.state.flow_fit = true;
        }
        let mut zoom = cx.state.flow_zoom;
        if ui
            .add(
                egui::Slider::new(&mut zoom, 0.35..=1.6)
                    .show_value(false)
                    .trailing_fill(true),
            )
            .on_hover_text("缩放（也可用滚轮）")
            .changed()
        {
            cx.state.flow_zoom = zoom;
        }
        widgets::hint(ui, &format!("{:.0}%", cx.state.flow_zoom * 100.0));
        ui.separator();
        widgets::hint(ui, "拖拽平移 · 滚轮缩放 · 点节点跳到对应条目");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (label, color) in [
                ("失败", theme::DANGER),
                ("生成中", theme::INFO),
                ("待生成", theme::TEXT_DIM),
                ("已生成", theme::SUCCESS),
            ] {
                widgets::pill(ui, label, color);
            }
        });
    });
}

fn build_graph(cx: &ViewCtx<'_>) -> Graph {
    let snapshot = cx.state.snapshot.as_ref().expect("checked by caller");
    let bd = snapshot.breakdown.as_ref().expect("checked by caller");
    let index = &snapshot.index;

    let mut nodes: Vec<Node> = Vec::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut rows = [0usize; 6];

    let push = |nodes: &mut Vec<Node>,
                    rows: &mut [usize; 6],
                    kind: NodeKind,
                    item_id: Option<String>,
                    title: String,
                    subtitle: String,
                    status: Option<ItemStatus>| {
        let col = kind.column();
        let row = rows[col];
        rows[col] += 1;
        nodes.push(Node {
            kind,
            item_id,
            title,
            subtitle,
            status,
            pos: Pos2::new(
                col as f32 * COL_WIDTH,
                row as f32 * (NODE_HEIGHT + ROW_GAP),
            ),
        });
        nodes.len() - 1
    };

    // 剧本 → 拆解
    let script_name = snapshot
        .script_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "未导入".into());
    let script = push(
        &mut nodes,
        &mut rows,
        NodeKind::Script,
        None,
        "剧本".into(),
        script_name,
        None,
    );
    let parse = push(
        &mut nodes,
        &mut rows,
        NodeKind::Parse,
        None,
        "拆解".into(),
        format!("{} 场 · {} 镜头", bd.scenes.len(), bd.shots.len()),
        None,
    );
    edges.push((script, parse));

    // 资产：每个角色 / 场景 / 道具一个节点
    let mut asset_index: Vec<(String, usize)> = Vec::new();
    for item in &index.assets {
        let kind = match item.kind {
            crate::model::index::ItemKind::Asset(k) => k,
            _ => continue,
        };
        let node = push(
            &mut nodes,
            &mut rows,
            NodeKind::Asset(kind),
            Some(item.id.clone()),
            item.title.clone(),
            kind.label().to_string(),
            Some(item.status),
        );
        edges.push((parse, node));
        asset_index.push((item.id.clone(), node));
    }

    // 分镜 / 视频：每个镜头各一个，并连出真实的参考关系
    for shot in &bd.shots {
        let frame = index.find(Stage::Storyboard, &shot.id);
        let clip = index.find(Stage::Video, &shot.id);
        let scene = bd.scene(&shot.scene_id).map(|s| s.number).unwrap_or(0);

        let frame_node = push(
            &mut nodes,
            &mut rows,
            NodeKind::Storyboard,
            Some(shot.id.clone()),
            shot.id.clone(),
            format!("第 {scene} 场 · {}", shot.framing),
            frame.map(|i| i.status),
        );

        // 该镜头引用了哪些资产，就从哪些资产连过来；没有就从拆解连。
        let mut linked = false;
        for cid in &shot.character_ids {
            if let Some((_, node)) = asset_index.iter().find(|(id, _)| id == cid) {
                edges.push((*node, frame_node));
                linked = true;
            }
        }
        if let Some(loc) = bd.location_for_shot(shot) {
            if let Some((_, node)) = asset_index.iter().find(|(id, _)| id == &loc.id) {
                edges.push((*node, frame_node));
                linked = true;
            }
        }
        for pid in &shot.prop_ids {
            if let Some((_, node)) = asset_index.iter().find(|(id, _)| id == pid) {
                edges.push((*node, frame_node));
                linked = true;
            }
        }
        if !linked {
            edges.push((parse, frame_node));
        }

        let clip_node = push(
            &mut nodes,
            &mut rows,
            NodeKind::Video,
            Some(shot.id.clone()),
            shot.id.clone(),
            format!("{} 秒", shot.duration_secs),
            clip.map(|i| i.status),
        );
        edges.push((frame_node, clip_node));
    }

    // 成片
    let export = push(
        &mut nodes,
        &mut rows,
        NodeKind::Export,
        None,
        "成片".into(),
        if index.final_cut.is_some() {
            "final.mp4 已生成".into()
        } else {
            "ffmpeg 拼接".to_string()
        },
        index
            .final_cut
            .is_some()
            .then_some(ItemStatus::Done)
            .or(Some(ItemStatus::Pending)),
    );
    for (i, node) in nodes.iter().enumerate() {
        if node.kind == NodeKind::Video {
            edges.push((i, export));
        }
    }

    // 最右一列也要完整放进来，所以宽度按「列间距 * (列数-1) + 节点宽」算。
    let width = 5.0 * COL_WIDTH + NODE_WIDTH;
    let height = rows
        .iter()
        .map(|r| *r as f32 * (NODE_HEIGHT + ROW_GAP))
        .fold(0.0, f32::max);
    Graph {
        nodes,
        edges,
        size: Vec2::new(width, height.max(NODE_HEIGHT)),
    }
}

fn canvas(ui: &mut Ui, cx: &mut ViewCtx<'_>, graph: &Graph) {
    let available = ui.available_size();
    let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
    let rect = response.rect;

    // 首次打开（或点「适应视图」）时缩放到整张图刚好放得下。
    if cx.state.flow_fit {
        let margin = 24.0;
        let scale_x = (rect.width() - margin * 2.0) / graph.size.x.max(1.0);
        let scale_y = (rect.height() - margin * 2.0) / graph.size.y.max(1.0);
        cx.state.flow_zoom = scale_x.min(scale_y).clamp(0.35, 1.0);
        cx.state.flow_pan = Vec2::splat(margin);
        cx.state.flow_fit = false;
    }

    painter.rect_filled(rect, theme::RADIUS_MD, theme::SURFACE);
    painter.rect_stroke(
        rect,
        theme::RADIUS_MD,
        Stroke::new(1.0_f32, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    // 缩放：以指针为中心
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            let old = cx.state.flow_zoom;
            let new = (old * (1.0 + scroll * 0.0015)).clamp(0.35, 1.6);
            if let Some(pointer) = response.hover_pos() {
                let local = pointer - rect.min - cx.state.flow_pan;
                cx.state.flow_pan -= local * (new / old - 1.0);
            }
            cx.state.flow_zoom = new;
        }
    }
    if response.dragged() {
        cx.state.flow_pan += response.drag_delta();
    }

    let zoom = cx.state.flow_zoom;
    let pan = cx.state.flow_pan;
    let to_screen = |p: Pos2| rect.min + pan + p.to_vec2() * zoom;
    let node_rect = |node: &Node| {
        Rect::from_min_size(
            to_screen(node.pos),
            Vec2::new(NODE_WIDTH, NODE_HEIGHT) * zoom,
        )
    };

    grid(&painter, rect, pan, zoom);

    // 连线先画，压在节点下面
    for (from, to) in &graph.edges {
        let a = node_rect(&graph.nodes[*from]);
        let b = node_rect(&graph.nodes[*to]);
        if !rect.intersects(a.union(b)) {
            continue;
        }
        let start = Pos2::new(a.right(), a.center().y);
        let end = Pos2::new(b.left(), b.center().y);
        let ctrl = 46.0 * zoom;
        painter.add(egui::Shape::CubicBezier(egui::epaint::CubicBezierShape {
            points: [
                start,
                Pos2::new(start.x + ctrl, start.y),
                Pos2::new(end.x - ctrl, end.y),
                end,
            ],
            closed: false,
            fill: Color32::TRANSPARENT,
            stroke: Stroke::new(1.2 * zoom, theme::tint(theme::BORDER_STRONG, 170)).into(),
        }));
    }

    // 节点
    let pointer = response.interact_pointer_pos();
    let mut clicked: Option<(NodeKind, String)> = None;
    let hover = response.hover_pos();

    for node in &graph.nodes {
        let r = node_rect(node);
        if !rect.intersects(r) {
            continue;
        }
        let status_color = node
            .status
            .map(theme::item_status_color)
            .unwrap_or(theme::TEXT_MUTED);
        let hovered = hover.is_some_and(|p| r.contains(p));

        painter.rect_filled(
            r,
            theme::RADIUS_SM,
            if hovered {
                theme::SURFACE_HOVER
            } else {
                theme::SURFACE_ALT
            },
        );
        painter.rect_stroke(
            r,
            theme::RADIUS_SM,
            Stroke::new(if hovered { 1.6_f32 } else { 1.0_f32 }, theme::tint(status_color, 150)),
            egui::StrokeKind::Inside,
        );
        // 左侧色条标明属于哪个阶段
        let bar = Rect::from_min_size(r.min, Vec2::new(4.0 * zoom, r.height()));
        painter.rect_filled(bar, 0.0, node.kind.accent());

        if zoom > 0.5 {
            painter.text(
                r.min + Vec2::new(12.0 * zoom, 8.0 * zoom),
                Align2::LEFT_TOP,
                truncate(&node.title, 16),
                FontId::proportional(12.0 * zoom),
                theme::TEXT,
            );
            painter.text(
                r.min + Vec2::new(12.0 * zoom, 26.0 * zoom),
                Align2::LEFT_TOP,
                truncate(&node.subtitle, 18),
                FontId::proportional(10.0 * zoom),
                theme::TEXT_DIM,
            );
        }
        painter.circle_filled(
            Pos2::new(r.right() - 10.0 * zoom, r.center().y),
            4.0 * zoom,
            status_color,
        );

        if response.clicked() && pointer.is_some_and(|p| r.contains(p)) {
            if let Some(id) = &node.item_id {
                clicked = Some((node.kind, id.clone()));
            } else if let Some(stage) = node.kind.stage() {
                cx.state.view = View::Stage(stage);
            } else {
                cx.state.view = View::Script;
            }
        }
    }

    if let Some((kind, id)) = clicked {
        if let Some(stage) = kind.stage() {
            if let Some(item) = cx.state.items(stage).iter().find(|i| i.id == id).cloned() {
                cx.state.select(stage, &item);
            }
            cx.state.view = View::Stage(stage);
        }
    }

    // 尺寸提示，方便知道图有多大
    painter.text(
        rect.right_bottom() - Vec2::new(10.0, 8.0),
        Align2::RIGHT_BOTTOM,
        format!("{} 个节点 · {} 条依赖", graph.nodes.len(), graph.edges.len()),
        FontId::proportional(11.0),
        theme::TEXT_DIM,
    );
}

fn grid(painter: &egui::Painter, rect: Rect, pan: Vec2, zoom: f32) {
    let step = 26.0 * zoom;
    if step < 10.0 {
        return;
    }
    let color = theme::tint(theme::BORDER, 90);
    let mut x = (pan.x % step + step) % step;
    while x < rect.width() {
        let px = rect.left() + x;
        painter.line_segment(
            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
            Stroke::new(1.0_f32, color),
        );
        x += step;
    }
    let mut y = (pan.y % step + step) % step;
    while y < rect.height() {
        let py = rect.top() + y;
        painter.line_segment(
            [Pos2::new(rect.left(), py), Pos2::new(rect.right(), py)],
            Stroke::new(1.0_f32, color),
        );
        y += step;
    }
}
