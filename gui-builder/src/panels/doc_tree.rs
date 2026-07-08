//! Document tree panel: classic left-aligned tree rows with per-depth
//! indentation, collapse triangles on containers, click-to-select,
//! drag-to-reorder with above/below/into drop zones, and a right-click
//! context menu (add child, delete, duplicate, wrap).

use crate::app::App;
use crate::doc_edit::{self, NodePath};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use petramond_ui::{Node, NodeKind};

const ROW_H: f32 = 18.0;
const INDENT: f32 = 14.0;
const CARET: Color32 = Color32::from_rgb(255, 200, 60);

enum Action {
    Select(NodePath),
    ToggleCollapse(NodePath),
    Delete(NodePath),
    Duplicate(NodePath),
    AddChild(NodePath, &'static str),
    Wrap(NodePath, bool),
    Drop { from: NodePath, parent: NodePath, index: usize },
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Document").strong());

    // An external selection must be reachable: expand its ancestors before
    // rendering so scroll_to_me can find the row.
    if app.tree_scroll_to_sel {
        if let Some(sel) = app.sel.clone() {
            for cut in 0..sel.len() {
                app.tree_collapsed.remove(&sel[..cut].to_vec());
            }
        }
    }

    let doc = app.proj.document.clone();
    let mut action = None;
    row(ui, app, &doc.root, Vec::new(), 0, &mut action);
    // External selections (canvas, validation…) scroll their row into view
    // exactly once.
    app.tree_scroll_to_sel = false;

    match action {
        None => {}
        Some(Action::Select(p)) => app.sel = Some(p),
        Some(Action::ToggleCollapse(p)) => {
            if !app.tree_collapsed.remove(&p) {
                app.tree_collapsed.insert(p);
            }
        }
        Some(Action::Delete(p)) => {
            app.sel = Some(p);
            app.delete_selected();
        }
        Some(Action::Duplicate(p)) => {
            app.sel = Some(p);
            app.duplicate_selected();
        }
        Some(Action::AddChild(p, ty)) => {
            if let Some(node) = doc_edit::new_node(&app.proj.document, ty) {
                app.sel = Some(p);
                app.insert_node(node);
            }
        }
        Some(Action::Wrap(p, as_row)) => {
            let kind = if as_row { NodeKind::Row } else { NodeKind::Column };
            app.mutate(|doc| {
                doc_edit::wrap_in(&mut doc.root, &p, kind);
            });
            app.sel = Some(p);
        }
        Some(Action::Drop { from, parent, index }) => apply_drop(app, from, parent, index),
    }
}

fn label_of(node: &Node) -> String {
    let mut s = node.kind.type_name().to_owned();
    if let Some(id) = &node.id {
        s.push_str(&format!(" #{id}"));
    }
    match &node.kind {
        NodeKind::Slot { role, .. } => s.push_str(&format!(" [{role}]")),
        NodeKind::SlotGrid { role, cols, rows, .. } => {
            s.push_str(&format!(" [{role} {cols}x{rows}]"))
        }
        NodeKind::Label { text, .. }
        | NodeKind::Button { text, .. }
        | NodeKind::Badge { text }
        | NodeKind::Alert { text, .. } => {
            if let Some(t) = text {
                let t: String = t.chars().take(18).collect();
                s.push_str(&format!(" \"{t}\""));
            }
        }
        _ => {}
    }
    s
}

/// Where a dragged row would land relative to the hovered row.
#[derive(Copy, Clone, PartialEq)]
enum Zone {
    Above,
    Below,
    Into,
}

fn drop_zone(rect: Rect, pointer_y: f32, is_container: bool, is_root: bool) -> Zone {
    if is_root {
        return Zone::Into;
    }
    let rel = ((pointer_y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    if is_container {
        // Top third = above, bottom third = below, middle third = reparent in.
        if rel < 1.0 / 3.0 {
            Zone::Above
        } else if rel > 2.0 / 3.0 {
            Zone::Below
        } else {
            Zone::Into
        }
    } else if rel < 0.5 {
        Zone::Above
    } else {
        Zone::Below
    }
}

fn row(
    ui: &mut egui::Ui,
    app: &App,
    node: &Node,
    path: NodePath,
    depth: usize,
    action: &mut Option<Action>,
) {
    let selected = app.sel.as_ref() == Some(&path);
    let is_container = node.kind.is_container();
    let has_children = !node.children.is_empty();
    let collapsed = app.tree_collapsed.contains(&path);

    // ONE full-width widget that senses click AND drag (a separate drag-only
    // overlay would swallow clicks in egui's hit test — the round-2 bug).
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click_and_drag());
    let indent = 4.0 + depth as f32 * INDENT;
    let text_x = rect.left() + indent + 13.0;

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect.expand2(Vec2::new(0.0, 1.0)));
        if selected {
            painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
        } else if response.hovered() {
            painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        if is_container && has_children {
            painter.text(
                Pos2::new(rect.left() + indent, rect.center().y),
                Align2::LEFT_CENTER,
                if collapsed { "▸" } else { "▾" },
                FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else if is_container {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        };
        painter.text(
            Pos2::new(text_x, rect.center().y),
            Align2::LEFT_CENTER,
            label_of(node),
            FontId::proportional(12.5),
            color,
        );
    }

    if response.clicked() || response.secondary_clicked() {
        *action = Some(Action::Select(path.clone()));
    }
    if response.drag_started() || response.dragged() {
        egui::DragAndDrop::set_payload(ui.ctx(), path.clone());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if selected && app.tree_scroll_to_sel {
        response.scroll_to_me(Some(egui::Align::Center));
    }

    // Collapse triangle: registered after the row so it wins clicks in its
    // little rect (it only senses click; drags still start the row's dnd).
    if is_container && has_children {
        let tri_rect = Rect::from_min_size(
            Pos2::new(rect.left() + indent - 4.0, rect.top()),
            Vec2::new(16.0, ROW_H),
        );
        let tri = ui.interact(tri_rect, egui::Id::new(("tree_tri", &path)), Sense::click());
        if tri.clicked() {
            *action = Some(Action::ToggleCollapse(path.clone()));
        }
    }

    // Drop target with above/below/into zones from the pointer's vertical
    // position; the caret draws at the actual insertion position (below the
    // LAST child's row inserts after it).
    if let Some(from) = response.dnd_hover_payload::<NodePath>() {
        if *from != path && !doc_edit::is_same_or_descendant(&from, &path) {
            let pointer_y = ui
                .ctx()
                .pointer_hover_pos()
                .map(|p| p.y)
                .unwrap_or(rect.center().y);
            let zone = drop_zone(rect, pointer_y, is_container, path.is_empty());
            let painter = ui.painter();
            match zone {
                Zone::Into => {
                    painter.rect_stroke(rect, 2.0, Stroke::new(1.5, CARET));
                }
                Zone::Above | Zone::Below => {
                    let y = if zone == Zone::Above { rect.top() } else { rect.bottom() };
                    painter.line_segment(
                        [Pos2::new(rect.left() + indent, y), Pos2::new(rect.right() - 2.0, y)],
                        Stroke::new(2.0, CARET),
                    );
                }
            }
            if let Some(from) = response.dnd_release_payload::<NodePath>() {
                let target = match zone {
                    Zone::Into => Some((path.clone(), node.children.len())),
                    Zone::Above if !path.is_empty() => {
                        Some((path[..path.len() - 1].to_vec(), *path.last().unwrap()))
                    }
                    Zone::Below if !path.is_empty() => {
                        Some((path[..path.len() - 1].to_vec(), *path.last().unwrap() + 1))
                    }
                    _ => None,
                };
                if let Some((parent, index)) = target {
                    *action = Some(Action::Drop { from: (*from).clone(), parent, index });
                }
            }
        }
    }

    response.context_menu(|ui| {
        if is_container {
            ui.menu_button("Add child", |ui| {
                for ty in doc_edit::NODE_TYPES {
                    if ui.button(*ty).clicked() {
                        *action = Some(Action::AddChild(path.clone(), ty));
                        ui.close_menu();
                    }
                }
            });
        }
        if !path.is_empty() {
            if ui.button("Duplicate").clicked() {
                *action = Some(Action::Duplicate(path.clone()));
                ui.close_menu();
            }
            if ui.button("Wrap in row").clicked() {
                *action = Some(Action::Wrap(path.clone(), true));
                ui.close_menu();
            }
            if ui.button("Wrap in column").clicked() {
                *action = Some(Action::Wrap(path.clone(), false));
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Delete").clicked() {
                *action = Some(Action::Delete(path.clone()));
                ui.close_menu();
            }
        }
    });

    if !collapsed {
        for (i, child) in node.children.iter().enumerate() {
            let mut p = path.clone();
            p.push(i);
            row(ui, app, child, p, depth + 1, action);
        }
    }
}

fn apply_drop(app: &mut App, from: NodePath, parent: NodePath, index: usize) {
    if from.is_empty() || doc_edit::is_same_or_descendant(&from, &parent) {
        return;
    }
    // Only containers take children.
    let ok = doc_edit::node_at(&app.proj.document.root, &parent)
        .is_some_and(|n| n.kind.is_container());
    if !ok {
        return;
    }
    // Same-slot moves are no-ops and don't deserve an undo entry.
    if parent.as_slice() == &from[..from.len() - 1] {
        let cur = *from.last().unwrap();
        if index == cur || index == cur + 1 {
            return;
        }
    }
    let mut new_sel = None;
    app.mutate(|doc| {
        new_sel = doc_edit::move_node(&mut doc.root, &from, &parent, index);
    });
    if new_sel.is_some() {
        app.sel = new_sel;
    }
}
