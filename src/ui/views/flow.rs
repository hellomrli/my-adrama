//! 流程图：把当前项目**真实的**依赖关系画出来。
//!
//! 排版规则（可读性优先）：
//! - 分镜与视频一一对应，放在**同一行**——两者之间是水平直线，不交叉。
//! - 镜头按**场次**分组、资产按**类别**分组，组有标题行。
//! - 剧本 / 拆解 / 成片相对整体高度垂直居中，而不是顶在左上角。
//! - 连线默认非常淡；**悬停或选中**某个节点时，只点亮与它相关的连线。
//!   多对多的「资产 → 分镜」依赖只有这样才看得清。
//!
//! 单击选中（高亮关联），双击跳到对应条目，点空白处取消选中。

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::ViewCtx;
use crate::model::{index::truncate, AssetKind, ItemStatus, Stage, ASSET_KINDS};
use crate::ui::state::View;
use crate::ui::{theme, widgets};

const COL: f32 = 240.0; // 列距
const NODE_W: f32 = 182.0;
const NODE_H: f32 = 46.0;
const ROW: f32 = NODE_H + 12.0; // 行距
const HEADER_H: f32 = 28.0; // 组标题行高
const GROUP_GAP: f32 = 16.0; // 组间距

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

    fn stage(self) -> Option<Stage> {
        match self {
            NodeKind::Script => None,
            NodeKind::Parse => Some(Stage::Parse),
            NodeKind::Asset(_) => Some(Stage::Assets),
            NodeKind::Storyboard => Some(Stage::Storyboard),
            NodeKind::Video | NodeKind::Export => Some(Stage::Video),
        }
    }
}

struct Node {
    kind: NodeKind,
    /// 稳定标识：悬停/选中状态跨帧、跨重建保持。
    key: String,
    item_id: Option<String>,
    title: String,
    subtitle: String,
    status: Option<ItemStatus>,
    /// 世界坐标（缩放前）。
    pos: Pos2,
}

/// 组标题（资产类别 / 场次），画在画布里。
struct Header {
    text: String,
    pos: Pos2,
    span: f32,
    color: Color32,
}

/// 连线端点：节点，或一个固定锚点（组标题）。
#[derive(Clone, Copy)]
enum End {
    N(usize),
    P(Pos2),
}

struct Graph {
    nodes: Vec<Node>,
    headers: Vec<Header>,
    edges: Vec<(End, End)>,
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
                egui::Slider::new(&mut zoom, 0.5..=1.6)
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
        widgets::hint(ui, "悬停/单击点亮关联 · 双击打开条目 · 拖拽平移 · 滚轮缩放");

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

// ---------------------------------------------------------------------------
// 布局
// ---------------------------------------------------------------------------

fn build_graph(cx: &ViewCtx<'_>) -> Graph {
    let snapshot = cx.state.snapshot.as_ref().expect("checked by caller");
    let bd = snapshot.breakdown.as_ref().expect("checked by caller");
    let index = &snapshot.index;

    // --- 先分组，量出两大块的高度，才能做垂直居中 ---
    let asset_groups: Vec<(AssetKind, Vec<&crate::model::ItemView>)> = ASSET_KINDS
        .into_iter()
        .map(|kind| {
            (
                kind,
                index
                    .assets
                    .iter()
                    .filter(|i| i.kind == crate::model::index::ItemKind::Asset(kind))
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, items)| !items.is_empty())
        .collect();

    let mut scene_groups: Vec<(String, Vec<&crate::model::Shot>)> = bd
        .scenes
        .iter()
        .map(|scene| {
            (
                format!("第 {} 场 · {}", scene.number, truncate(&scene.title, 8)),
                bd.shots_in_scene(scene.number),
            )
        })
        .filter(|(_, shots)| !shots.is_empty())
        .collect();
    let orphans: Vec<&crate::model::Shot> = bd
        .shots
        .iter()
        .filter(|s| bd.scene(&s.scene_id).is_none())
        .collect();
    if !orphans.is_empty() {
        scene_groups.push(("未归入场次".into(), orphans));
    }

    let block_height = |groups: usize, rows: usize| -> f32 {
        if groups == 0 {
            return 0.0;
        }
        groups as f32 * HEADER_H
            + rows as f32 * ROW
            + (groups.saturating_sub(1)) as f32 * GROUP_GAP
    };
    let assets_h = block_height(
        asset_groups.len(),
        asset_groups.iter().map(|(_, v)| v.len()).sum(),
    );
    let shots_h = block_height(
        scene_groups.len(),
        scene_groups.iter().map(|(_, v)| v.len()).sum(),
    );
    let total_h = assets_h.max(shots_h).max(NODE_H);

    // --- 构建 ---
    let mut nodes: Vec<Node> = Vec::new();
    let mut headers: Vec<Header> = Vec::new();
    let mut edges: Vec<(End, End)> = Vec::new();

    let center_y = total_h / 2.0 - NODE_H / 2.0;
    let x = |col: usize| col as f32 * COL;

    // 剧本 → 拆解（垂直居中）
    let script_name = snapshot
        .script_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "未导入".into());
    nodes.push(Node {
        kind: NodeKind::Script,
        key: "script".into(),
        item_id: None,
        title: "剧本".into(),
        subtitle: truncate(&script_name, 16),
        status: None,
        pos: Pos2::new(x(0), center_y),
    });
    nodes.push(Node {
        kind: NodeKind::Parse,
        key: "parse".into(),
        item_id: None,
        title: "拆解".into(),
        subtitle: format!("{} 场 · {} 镜头", bd.scenes.len(), bd.shots.len()),
        status: None,
        pos: Pos2::new(x(1), center_y),
    });
    let parse = 1usize;
    edges.push((End::N(0), End::N(parse)));

    // 资产块（垂直居中；拆解只连到组标题，不连每个节点——那是噪音）
    let mut asset_idx: Vec<(String, usize)> = Vec::new();
    let mut y = (total_h - assets_h) / 2.0;
    for (kind, items) in &asset_groups {
        let anchor = Pos2::new(x(2) - 10.0, y + HEADER_H * 0.5);
        headers.push(Header {
            text: format!("{}（{}）", kind.label(), items.len()),
            pos: Pos2::new(x(2), y),
            span: NODE_W,
            color: theme::stage_color(Stage::Assets),
        });
        edges.push((End::N(parse), End::P(anchor)));
        y += HEADER_H;
        for item in items {
            nodes.push(Node {
                kind: NodeKind::Asset(*kind),
                key: format!("asset:{}", item.id),
                item_id: Some(item.id.clone()),
                title: truncate(&item.title, 14),
                subtitle: truncate(&item.subtitle, 15),
                status: Some(item.status),
                pos: Pos2::new(x(2), y),
            });
            asset_idx.push((item.id.clone(), nodes.len() - 1));
            y += ROW;
        }
        y += GROUP_GAP;
    }

    // 镜头块：分镜与视频同一行，场次分组
    let mut y = (total_h - shots_h) / 2.0;
    for (title, shots) in &scene_groups {
        headers.push(Header {
            text: title.clone(),
            pos: Pos2::new(x(3), y),
            span: COL + NODE_W,
            color: theme::stage_color(Stage::Storyboard),
        });
        y += HEADER_H;
        for shot in shots {
            let frame = snapshot.index.find(Stage::Storyboard, &shot.id);
            let clip = snapshot.index.find(Stage::Video, &shot.id);

            nodes.push(Node {
                kind: NodeKind::Storyboard,
                key: format!("sb:{}", shot.id),
                item_id: Some(shot.id.clone()),
                title: truncate(&shot.id, 16),
                subtitle: truncate(&shot.visual, 15),
                status: Some(frame.map(|i| i.status).unwrap_or_default()),
                pos: Pos2::new(x(3), y),
            });
            let sb = nodes.len() - 1;

            nodes.push(Node {
                kind: NodeKind::Video,
                key: format!("vid:{}", shot.id),
                item_id: Some(shot.id.clone()),
                title: truncate(&shot.id, 16),
                subtitle: format!(
                    "{} 秒 · {}",
                    shot.duration_secs,
                    truncate(&shot.framing, 8)
                ),
                status: Some(clip.map(|i| i.status).unwrap_or_default()),
                pos: Pos2::new(x(4), y),
            });
            let vid = nodes.len() - 1;
            edges.push((End::N(sb), End::N(vid)));

            // 真实的参考关系：该镜头引用了哪些资产
            let mut linked = false;
            let link = |edges: &mut Vec<(End, End)>, id: &str| {
                if let Some((_, idx)) = asset_idx.iter().find(|(aid, _)| aid == id) {
                    edges.push((End::N(*idx), End::N(sb)));
                    true
                } else {
                    false
                }
            };
            for cid in &shot.character_ids {
                linked |= link(&mut edges, cid);
            }
            if let Some(loc) = bd.location_for_shot(shot) {
                linked |= link(&mut edges, &loc.id);
            }
            for pid in &shot.prop_ids {
                linked |= link(&mut edges, pid);
            }
            if !linked {
                edges.push((End::N(parse), End::N(sb)));
            }
            y += ROW;
        }
        y += GROUP_GAP;
    }

    // 成片（垂直居中）
    nodes.push(Node {
        kind: NodeKind::Export,
        key: "export".into(),
        item_id: None,
        title: "成片".into(),
        subtitle: if index.final_cut.is_some() {
            "final.mp4 已生成".into()
        } else {
            "ffmpeg 拼接".into()
        },
        status: Some(if index.final_cut.is_some() {
            ItemStatus::Done
        } else {
            ItemStatus::Pending
        }),
        pos: Pos2::new(x(5), center_y),
    });
    let export = nodes.len() - 1;
    let video_nodes: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NodeKind::Video)
        .map(|(i, _)| i)
        .collect();
    for i in video_nodes {
        edges.push((End::N(i), End::N(export)));
    }

    Graph {
        nodes,
        headers,
        edges,
        size: Vec2::new(5.0 * COL + NODE_W, total_h),
    }
}

// ---------------------------------------------------------------------------
// 绘制
// ---------------------------------------------------------------------------

fn canvas(ui: &mut Ui, cx: &mut ViewCtx<'_>, graph: &Graph) {
    let available = ui.available_size();
    let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
    let rect = response.rect;

    painter.rect_filled(rect, theme::RADIUS_MD, theme::SURFACE);
    painter.rect_stroke(
        rect,
        theme::RADIUS_MD,
        Stroke::new(1.0_f32, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    // 适应视图：整张图刚好放下并居中；最小 0.5 保证文字始终可读
    if cx.state.flow_fit {
        let z = ((rect.width() - 48.0) / graph.size.x.max(1.0))
            .min((rect.height() - 32.0) / graph.size.y.max(1.0))
            .clamp(0.5, 1.0);
        cx.state.flow_zoom = z;
        cx.state.flow_pan = Vec2::new(
            ((rect.width() - graph.size.x * z) / 2.0).max(24.0),
            ((rect.height() - graph.size.y * z) / 2.0).max(16.0),
        );
        cx.state.flow_fit = false;
    }

    // 缩放以指针为中心
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            let old = cx.state.flow_zoom;
            let new = (old * (1.0 + scroll * 0.0015)).clamp(0.5, 1.6);
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

    let z = cx.state.flow_zoom;
    let pan = cx.state.flow_pan;
    let world = |p: Pos2| rect.min + pan + p.to_vec2() * z;
    let node_rect = |n: &Node| Rect::from_min_size(world(n.pos), Vec2::new(NODE_W, NODE_H) * z);

    grid(&painter, rect, pan, z);

    // --- 悬停 / 选中 ---
    let hover_idx = response
        .hover_pos()
        .and_then(|p| graph.nodes.iter().position(|n| node_rect(n).contains(p)));
    let selected_key = cx.state.flow_selected.clone();
    let active_key = hover_idx
        .map(|i| graph.nodes[i].key.clone())
        .or(selected_key.clone());
    let active_idx = active_key
        .as_ref()
        .and_then(|k| graph.nodes.iter().position(|n| &n.key == k));

    // --- 组标题 ---
    for header in &graph.headers {
        let p = world(header.pos);
        painter.text(
            p + Vec2::new(2.0 * z, 2.0 * z),
            Align2::LEFT_TOP,
            &header.text,
            FontId::proportional(11.5 * z),
            theme::TEXT_MUTED,
        );
        let line_y = p.y + (HEADER_H - 8.0) * z;
        painter.line_segment(
            [
                Pos2::new(p.x, line_y),
                Pos2::new(p.x + header.span * z, line_y),
            ],
            Stroke::new(1.0_f32, theme::tint(header.color, 110)),
        );
    }

    // --- 连线：先淡的，再亮的（保证高亮在上层） ---
    let end_point = |end: &End, outgoing: bool| -> Pos2 {
        match end {
            End::N(i) => {
                let r = node_rect(&graph.nodes[*i]);
                if outgoing {
                    Pos2::new(r.right(), r.center().y)
                } else {
                    Pos2::new(r.left(), r.center().y)
                }
            }
            End::P(p) => world(*p),
        }
    };
    let touches_active = |end: &End| -> bool {
        match (end, active_idx) {
            (End::N(i), Some(a)) => *i == a,
            _ => false,
        }
    };

    let mut highlighted: Vec<(Pos2, Pos2)> = Vec::new();
    for (a, b) in &graph.edges {
        let from = end_point(a, true);
        let to = end_point(b, false);
        if !rect.intersects(Rect::from_two_pos(from, to).expand(40.0)) {
            continue;
        }
        if touches_active(a) || touches_active(b) {
            highlighted.push((from, to));
            continue;
        }
        // 有激活节点时，无关的线再淡一档
        let alpha = if active_idx.is_some() { 26 } else { 56 };
        draw_edge(
            &painter,
            from,
            to,
            Stroke::new(1.1 * z, theme::tint(theme::BORDER_STRONG, alpha)),
            false,
            z,
        );
    }
    let accent = active_idx
        .map(|i| graph.nodes[i].kind.accent())
        .unwrap_or(theme::ACCENT);
    for (from, to) in &highlighted {
        draw_edge(
            &painter,
            *from,
            *to,
            Stroke::new(2.0 * z, theme::tint(accent, 220)),
            true,
            z,
        );
    }

    // --- 节点 ---
    let pointer = response.interact_pointer_pos();
    let mut open_item: Option<usize> = None;
    let mut clicked_node: Option<usize> = None;

    for (i, node) in graph.nodes.iter().enumerate() {
        let r = node_rect(node);
        if !rect.intersects(r) {
            continue;
        }
        let is_hovered = hover_idx == Some(i);
        let is_selected = selected_key.as_deref() == Some(node.key.as_str());
        let is_active = active_idx == Some(i);
        let status_color = node
            .status
            .map(theme::item_status_color)
            .unwrap_or(theme::TEXT_MUTED);

        painter.rect_filled(
            r,
            theme::RADIUS_SM,
            if is_hovered {
                theme::SURFACE_HOVER
            } else {
                theme::SURFACE_ALT
            },
        );
        painter.rect_stroke(
            r,
            theme::RADIUS_SM,
            if is_selected {
                Stroke::new(1.6_f32, node.kind.accent())
            } else if is_active {
                Stroke::new(1.3_f32, theme::tint(node.kind.accent(), 180))
            } else {
                Stroke::new(1.0_f32, theme::tint(theme::BORDER_STRONG, 130))
            },
            egui::StrokeKind::Inside,
        );
        // 左侧色条标明阶段
        painter.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(3.5 * z, r.height())),
            0.0,
            node.kind.accent(),
        );

        painter.text(
            r.min + Vec2::new(12.0 * z, 8.0 * z),
            Align2::LEFT_TOP,
            &node.title,
            FontId::proportional(12.5 * z),
            theme::TEXT,
        );
        painter.text(
            r.min + Vec2::new(12.0 * z, 26.0 * z),
            Align2::LEFT_TOP,
            &node.subtitle,
            FontId::proportional(10.5 * z),
            theme::TEXT_DIM,
        );
        painter.circle_filled(
            Pos2::new(r.right() - 10.0 * z, r.min.y + 12.0 * z),
            3.5 * z,
            status_color,
        );

        if pointer.is_some_and(|p| r.contains(p)) {
            if response.double_clicked() {
                open_item = Some(i);
            } else if response.clicked() {
                clicked_node = Some(i);
            }
        }
    }

    // 点空白处取消选中
    if response.clicked() && clicked_node.is_none() && hover_idx.is_none() {
        cx.state.flow_selected = None;
    }
    if let Some(i) = clicked_node {
        let key = graph.nodes[i].key.clone();
        cx.state.flow_selected = if cx.state.flow_selected.as_deref() == Some(key.as_str()) {
            None
        } else {
            Some(key)
        };
    }
    if let Some(i) = open_item {
        navigate_to(cx, &graph.nodes[i]);
    }

    painter.text(
        rect.right_bottom() - Vec2::new(10.0, 8.0),
        Align2::RIGHT_BOTTOM,
        format!("{} 个节点 · {} 条依赖", graph.nodes.len(), graph.edges.len()),
        FontId::proportional(11.0),
        theme::TEXT_DIM,
    );
}

fn navigate_to(cx: &mut ViewCtx<'_>, node: &Node) {
    match (&node.item_id, node.kind.stage()) {
        (Some(id), Some(stage)) => {
            if let Some(item) = cx.state.items(stage).iter().find(|i| &i.id == id).cloned() {
                cx.state.select(stage, &item);
            }
            cx.state.view = View::Stage(stage);
        }
        (None, Some(stage)) => cx.state.view = View::Stage(stage),
        (None, None) => cx.state.view = View::Script,
        _ => {}
    }
}

fn draw_edge(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke, arrow: bool, z: f32) {
    let dx = ((to.x - from.x).abs() * 0.45).max(24.0 * z);
    painter.add(egui::Shape::CubicBezier(egui::epaint::CubicBezierShape {
        points: [
            from,
            Pos2::new(from.x + dx, from.y),
            Pos2::new(to.x - dx, to.y),
            to,
        ],
        closed: false,
        fill: Color32::TRANSPARENT,
        stroke: stroke.into(),
    }));
    if arrow {
        let s = 4.5 * z;
        painter.add(egui::Shape::convex_polygon(
            vec![
                to,
                Pos2::new(to.x - 2.0 * s, to.y - s),
                Pos2::new(to.x - 2.0 * s, to.y + s),
            ],
            stroke.color,
            Stroke::NONE,
        ));
    }
}

fn grid(painter: &egui::Painter, rect: Rect, pan: Vec2, zoom: f32) {
    let step = 26.0 * zoom;
    if step < 10.0 {
        return;
    }
    let color = theme::tint(theme::BORDER, 70);
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
