//! egui panels: top bar (File/Edit menus + type/resolution/zoom), left panel
//! (layer tree, slots, inspector), right panel (asset palette), snap dialog.
//!
//! The layer tree supports groups, inline rename (double-click), a right-click
//! context menu (Duplicate / Rename / Ungroup / Delete) and drag-to-reorder
//! (including moving layers in and out of groups). Project mutations call
//! `app.begin_edit()` so they're undoable.

use crate::app::{App, DragPayload, PanelItem, Selection, TagDialog};
use crate::icons;
use crate::model::{AssetSpec, Container, DropTarget, Grid, GuiType, Hover, Layer, LayerFit, LayerTag, Node, RectF, Slot, SlotRole};
use eframe::egui::{self, Color32, Id, LayerId, Order, Pos2, Rect, RichText, Sense, Stroke};
use std::collections::HashSet;

fn rtl() -> egui::Layout {
    egui::Layout::right_to_left(egui::Align::Center)
}

fn eye_button(ui: &mut egui::Ui, visible: bool) -> bool {
    let (glyph, col, hint) = if visible {
        (icons::EYE, Color32::from_gray(235), "Visible — click to hide")
    } else {
        (icons::EYE_OFF, Color32::from_gray(120), "Hidden — click to show")
    };
    ui.add(egui::Button::new(RichText::new(glyph).color(col).size(16.0)).frame(false))
        .on_hover_text(hint)
        .clicked()
}

/// A small colored chip showing a layer's tag (e.g. next to its name in the
/// panel). Uses a text background so it needs no Frame/Margin API.
fn tag_badge(ui: &mut egui::Ui, tag: LayerTag) {
    let c = tag.badge_color();
    let chip = RichText::new(format!(" {} ", tag.short()))
        .small()
        .strong()
        .color(Color32::from_gray(20))
        .background_color(Color32::from_rgb(c[0], c[1], c[2]));
    ui.add(egui::Label::new(chip)).on_hover_text(tag.label());
}

// ===========================================================================
// Top bar
// ===========================================================================

pub fn top_bar(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("topbar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            file_menu(app, ui, ctx);
            edit_menu(app, ui);
            ui.separator();

            ui.label("Type");
            let mut ty = app.project.gui_type;
            egui::ComboBox::from_id_salt("gui_type").selected_text(ty.label()).show_ui(ui, |ui| {
                for t in GuiType::ALL {
                    ui.selectable_value(&mut ty, t, t.label());
                }
            });
            if ty != app.project.gui_type {
                app.begin_edit();
                app.project.gui_type = ty;
                app.project.resize_to_type_scale();
            }

            if app.project.gui_type.aspect_locked() {
                ui.label("Resolution");
                let mut s = app.project.scale.max(1);
                egui::ComboBox::from_id_salt("scale").selected_text(format!("{s}×")).show_ui(ui, |ui| {
                    for v in 1..=8u32 {
                        ui.selectable_value(&mut s, v, format!("{v}×"));
                    }
                });
                if s != app.project.scale {
                    app.begin_edit();
                    app.project.scale = s;
                    app.project.resize_to_type_scale();
                }
            } else {
                ui.label("W");
                if ui.add(egui::DragValue::new(&mut app.project.canvas.w).speed(1.0)).changed() {
                    app.begin_edit();
                }
                ui.label("H");
                if ui.add(egui::DragValue::new(&mut app.project.canvas.h).speed(1.0)).changed() {
                    app.begin_edit();
                }
                app.project.canvas.w = app.project.canvas.w.max(1);
                app.project.canvas.h = app.project.canvas.h.max(1);
            }
            ui.label(RichText::new(format!("{}×{} px", app.project.canvas.w, app.project.canvas.h)).weak());

            ui.separator();
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut app.view.zoom, 0.1..=8.0).logarithmic(true).show_value(false));
            if ui.button("Fit").clicked() {
                app.view.zoom = 1.0;
                app.view.pan = egui::Vec2::ZERO;
            }
            ui.separator();
            let s = app.show_slots;
            if eye_button(ui, s) {
                app.show_slots = !s;
            }
            ui.label("Slots");
        });
    });
}

fn file_menu(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.menu_button("File", |ui| {
        if ui.button("New").clicked() {
            app.new_project();
            ui.close_menu();
        }
        if ui.button("Open .llgui…").clicked() {
            ui.close_menu();
            app.open(ctx);
        }
        ui.separator();
        if ui.button("Save").clicked() {
            ui.close_menu();
            app.save(ctx);
        }
        if ui.button("Save As…").clicked() {
            ui.close_menu();
            app.save_as(ctx);
        }
        ui.separator();
        if ui.button("Bake PNG + JSON…").clicked() {
            ui.close_menu();
            app.bake(ctx);
        }
        ui.separator();
        if ui.button("Quit").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
}

fn edit_menu(app: &mut App, ui: &mut egui::Ui) {
    ui.menu_button("Edit", |ui| {
        if ui.add_enabled(app.can_undo(), egui::Button::new("Undo   Ctrl+Z")).clicked() {
            app.undo();
            ui.close_menu();
        }
        if ui.add_enabled(app.can_redo(), egui::Button::new("Redo   Ctrl+Shift+Z")).clicked() {
            app.redo();
            ui.close_menu();
        }
        ui.separator();
        ui.checkbox(&mut app.snap_enabled, "Snap to grid");
        if ui.button(format!("Snapping resolution…  ({} px)", app.snap_step)).clicked() {
            app.show_snap_dialog = true;
            ui.close_menu();
        }
        ui.label(RichText::new("Ctrl while dragging bypasses snap.").weak().small());
        ui.label(RichText::new("Shift while dragging locks an axis.").weak().small());
    });
}

pub fn snap_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.show_snap_dialog {
        return;
    }
    let mut open = app.show_snap_dialog;
    egui::Window::new("Snapping resolution").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.checkbox(&mut app.snap_enabled, "Snap to grid enabled");
        ui.add_space(6.0);
        ui.label("Grid step (pixels):");
        ui.add(egui::Slider::new(&mut app.snap_step, 1..=64));
        ui.horizontal(|ui| {
            ui.label("Exact:");
            ui.add(egui::DragValue::new(&mut app.snap_step).speed(1.0));
        });
        ui.horizontal(|ui| {
            for preset in [1, 2, 4, 8, 16, 18] {
                if ui.small_button(preset.to_string()).clicked() {
                    app.snap_step = preset;
                }
            }
        });
        app.snap_step = app.snap_step.clamp(1, 1024);
    });
    app.show_snap_dialog = open;
}

/// Dialog to tag the selected layer with a predefined dynamic-overlay role (or
/// clear it). A tag is unique project-wide, so OK *moves* it off whatever layer
/// held it before. Tagged layers bake to their own PNG instead of the panel.
pub fn tag_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut state) = app.tag_dialog.clone() else {
        return;
    };
    let Some(layer_name) = app.project.layer(state.layer_id).map(|l| l.name.clone()) else {
        app.tag_dialog = None;
        return;
    };

    let mut window_open = true;
    let mut do_ok = false;
    let mut do_cancel = false;
    egui::Window::new("Tag layer")
        .collapsible(false)
        .resizable(false)
        .open(&mut window_open)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(RichText::new(format!("Layer: {layer_name}")).strong());
            ui.label(
                RichText::new("Tagged layers are baked as their own PNG so the game can drive them at runtime (e.g. fill the arrow by smelt progress).")
                    .weak()
                    .small(),
            );
            ui.add_space(8.0);
            ui.radio_value(&mut state.choice, None, "None (untag)");
            for t in LayerTag::ALL {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut state.choice, Some(t), t.label());
                    tag_badge(ui, t);
                    if let Some(owner) = app.project.layer_with_tag(t) {
                        if owner != state.layer_id {
                            let owner_name = app.project.layer(owner).map(|l| l.name.clone()).unwrap_or_default();
                            ui.label(RichText::new(format!("on “{owner_name}” — will move here")).weak().small());
                        }
                    }
                });
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    do_ok = true;
                }
                if ui.button("Cancel").clicked() {
                    do_cancel = true;
                }
            });
        });

    if do_ok {
        app.set_layer_tag(state.layer_id, state.choice);
        app.tag_dialog = None;
    } else if do_cancel || !window_open {
        app.tag_dialog = None;
    } else {
        app.tag_dialog = Some(state);
    }
}

// ===========================================================================
// Left panel: layer tree, slots, inspector
// ===========================================================================

pub fn left_panel(app: &mut App, ctx: &egui::Context) {
    egui::SidePanel::left("left").resizable(true).default_width(290.0).show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Layers");
                ui.with_layout(rtl(), |ui| {
                    if ui.button(format!("{} Group", icons::PLUS)).clicked() {
                        app.add_group();
                    }
                });
            });
            ui.label(RichText::new("top = back · drag to reorder · dbl-click to rename · right-click for menu").weak().small());
            tree_panel(app, ui);

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("Slots");
                ui.with_layout(rtl(), |ui| {
                    if ui.button(format!("{} Slot", icons::PLUS)).clicked() {
                        add_slot_center(app);
                    }
                });
            });
            slots_list(app, ui);

            ui.add_space(8.0);
            ui.separator();
            ui.heading("Hover highlight");
            hover_panel(app, ui);

            ui.add_space(8.0);
            ui.separator();
            ui.heading("Inspector");
            inspector(app, ui);
        });
    });
}

enum TreeOp {
    Select(Selection),
    ToggleVis(u64, bool),
    ToggleCollapse(u64),
    StartDrag(PanelItem),
    StartRename(u64),
    CommitRename(u64),
    CancelRename,
    Duplicate(u64),
    Delete(u64),
    Ungroup(u64),
    Tag(u64),
}

/// A flattened, owned snapshot of one visible tree row (built before rendering
/// so the panel doesn't borrow the project while it mutates rename state).
struct RenderRow {
    depth: usize,
    container: Container,
    index: usize,
    id: u64,
    name: String,
    visible: bool,
    is_group: bool,
    collapsed: bool,
    tag: Option<LayerTag>,
}

fn build_rows(nodes: &[Node], container: Container, depth: usize, out: &mut Vec<RenderRow>) {
    for (i, n) in nodes.iter().enumerate() {
        match n {
            Node::Layer(l) => out.push(RenderRow {
                depth,
                container,
                index: i,
                id: l.id,
                name: l.name.clone(),
                visible: l.visible,
                is_group: false,
                collapsed: false,
                tag: l.tag,
            }),
            Node::Group(g) => {
                out.push(RenderRow {
                    depth,
                    container,
                    index: i,
                    id: g.id,
                    name: g.name.clone(),
                    visible: g.visible,
                    is_group: true,
                    collapsed: g.collapsed,
                    tag: None,
                });
                if !g.collapsed {
                    build_rows(&g.children, Container::Group(g.id), depth + 1, out);
                }
            }
        }
    }
}

fn tree_panel(app: &mut App, ui: &mut egui::Ui) {
    let sel = app.selection;
    let dragging = app.panel_drag;
    let renaming = app.renaming;
    let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
    let released = ui.ctx().input(|i| i.pointer.any_released());
    let drag_name = dragging.and_then(|d| {
        app.project.group(d.id).map(|g| g.name.clone()).or_else(|| app.project.layer(d.id).map(|l| l.name.clone()))
    });
    // When dragging a group, its own subtree is not a valid drop destination.
    let forbidden: HashSet<u64> = match dragging {
        Some(d) if d.is_group => app.project.subtree_ids(d.id).into_iter().collect(),
        _ => HashSet::new(),
    };
    let top_len = app.project.nodes.len();

    let mut rows = Vec::new();
    build_rows(&app.project.nodes, Container::Top, 0, &mut rows);

    let mut ops: Vec<TreeOp> = Vec::new();
    let mut drop: Option<DropTarget> = None;
    let mut line: Option<(f32, f32)> = None; // (y, indent x)
    let mut last_bottom = ui.min_rect().top();

    for row in &rows {
        let rect = if row.is_group {
            render_group_header(app, ui, sel, renaming, row.id, &row.name, row.visible, row.collapsed, row.depth, &mut ops)
        } else {
            render_layer_row(app, ui, sel, renaming, row.id, &row.name, row.visible, row.tag, row.depth, &mut ops)
        };
        last_bottom = rect.bottom();
        consider_drop(row, rect, dragging, &forbidden, pointer, &mut drop, &mut line);
    }
    if rows.is_empty() {
        ui.label(RichText::new("No layers. Drag a part from the right.").weak().small());
    }

    // Dropping in the empty space below the list appends to the top level.
    if let (Some(_), Some(p)) = (dragging, pointer) {
        if drop.is_none() && p.y > last_bottom {
            drop = Some(DropTarget { container: Container::Top, index: top_len });
            line = Some((last_bottom, ui.min_rect().left()));
        }
    }

    if let Some((y, x0)) = line {
        let r = ui.min_rect();
        ui.painter().hline(egui::Rangef::new(x0, r.right()), y, Stroke::new(2.0, Color32::from_rgb(250, 220, 90)));
    }
    if dragging.is_some() {
        if let (Some(p), Some(name)) = (pointer, &drag_name) {
            let gp = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new("panel_ghost")));
            gp.text(p + egui::vec2(14.0, 2.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(13.0), Color32::WHITE);
        }
    }

    for op in ops {
        apply_tree_op(app, op);
    }
    if released {
        if let (Some(d), Some(t)) = (dragging, drop) {
            app.move_panel(d.id, t);
        }
        app.panel_drag = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_layer_row(app: &mut App, ui: &mut egui::Ui, sel: Option<Selection>, renaming: Option<u64>, id: u64, name: &str, visible: bool, tag: Option<LayerTag>, depth: usize, ops: &mut Vec<TreeOp>) -> Rect {
    ui.horizontal(|ui| {
        if depth > 0 {
            ui.add_space(depth as f32 * 16.0);
        }
        if eye_button(ui, visible) {
            ops.push(TreeOp::ToggleVis(id, false));
        }
        row_name(app, ui, sel, renaming, id, name, false, tag, ops);
    })
    .response
    .rect
}

#[allow(clippy::too_many_arguments)]
fn render_group_header(app: &mut App, ui: &mut egui::Ui, sel: Option<Selection>, renaming: Option<u64>, id: u64, name: &str, visible: bool, collapsed: bool, depth: usize, ops: &mut Vec<TreeOp>) -> Rect {
    ui.horizontal(|ui| {
        if depth > 0 {
            ui.add_space(depth as f32 * 16.0);
        }
        let tri = if collapsed { icons::CHEVRON_RIGHT } else { icons::CHEVRON_DOWN };
        if ui.add(egui::Label::new(RichText::new(tri).size(14.0)).sense(Sense::click())).clicked() {
            ops.push(TreeOp::ToggleCollapse(id));
        }
        if eye_button(ui, visible) {
            ops.push(TreeOp::ToggleVis(id, true));
        }
        row_name(app, ui, sel, renaming, id, name, true, None, ops);
    })
    .response
    .rect
}

#[allow(clippy::too_many_arguments)]
fn row_name(app: &mut App, ui: &mut egui::Ui, sel: Option<Selection>, renaming: Option<u64>, id: u64, name: &str, is_group: bool, tag: Option<LayerTag>, ops: &mut Vec<TreeOp>) {
    if renaming == Some(id) {
        let resp = ui.add(egui::TextEdit::singleline(&mut app.rename_buf).desired_width(150.0));
        if app.rename_focus {
            resp.request_focus();
            app.rename_focus = false;
        }
        if resp.lost_focus() {
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            ops.push(if esc { TreeOp::CancelRename } else { TreeOp::CommitRename(id) });
        }
        return;
    }

    let selected = if is_group {
        matches!(sel, Some(Selection::Group(s)) if s == id)
    } else {
        matches!(sel, Some(Selection::Layer(s)) if s == id)
    };
    let mut txt = RichText::new(name);
    if is_group {
        txt = txt.strong();
    }
    if selected {
        txt = txt.background_color(Color32::from_rgb(60, 70, 95));
    }
    let resp = ui.add(egui::Label::new(txt).sense(Sense::click_and_drag()));
    if let Some(t) = tag {
        tag_badge(ui, t);
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        ops.push(TreeOp::StartDrag(PanelItem { id, is_group }));
    } else if resp.double_clicked() {
        ops.push(TreeOp::StartRename(id));
    } else if resp.clicked() {
        ops.push(TreeOp::Select(if is_group { Selection::Group(id) } else { Selection::Layer(id) }));
    }
    resp.context_menu(|ui| {
        if ui.button("Duplicate").clicked() {
            ops.push(TreeOp::Duplicate(id));
            ui.close_menu();
        }
        if ui.button("Rename").clicked() {
            ops.push(TreeOp::StartRename(id));
            ui.close_menu();
        }
        if !is_group {
            let tag_label = if tag.is_some() { "Tag…  (set)" } else { "Tag…" };
            if ui.button(tag_label).clicked() {
                ops.push(TreeOp::Tag(id));
                ui.close_menu();
            }
        }
        if is_group && ui.button("Ungroup").clicked() {
            ops.push(TreeOp::Ungroup(id));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            ops.push(TreeOp::Delete(id));
            ui.close_menu();
        }
    });
}

fn consider_drop(row: &RenderRow, rect: Rect, dragging: Option<PanelItem>, forbidden: &HashSet<u64>, pointer: Option<Pos2>, drop: &mut Option<DropTarget>, line: &mut Option<(f32, f32)>) {
    if dragging.is_none() {
        return;
    }
    let Some(p) = pointer else {
        return;
    };
    if !rect.contains(p) {
        return;
    }
    // Can't drop a group into/among its own subtree.
    if forbidden.contains(&row.id) {
        return;
    }
    if let Container::Group(c) = row.container {
        if forbidden.contains(&c) {
            return;
        }
    }
    let before = p.y < rect.center().y;
    let indent_x = rect.left() + row.depth as f32 * 16.0;
    if row.is_group {
        if before {
            // Reorder before this group within its container.
            *drop = Some(DropTarget { container: row.container, index: row.index });
            *line = Some((rect.top(), indent_x));
        } else {
            // Drop into this group (at its front). Works for layers and groups
            // (nesting); the model rejects cycles as a safety net.
            *drop = Some(DropTarget { container: Container::Group(row.id), index: 0 });
            *line = Some((rect.bottom(), indent_x + 16.0));
        }
    } else {
        let index = if before { row.index } else { row.index + 1 };
        *drop = Some(DropTarget { container: row.container, index });
        *line = Some((if before { rect.top() } else { rect.bottom() }, indent_x));
    }
}

fn apply_tree_op(app: &mut App, op: TreeOp) {
    match op {
        TreeOp::Select(s) => app.selection = Some(s),
        TreeOp::ToggleVis(id, is_group) => {
            app.begin_edit();
            if is_group {
                if let Some(g) = app.project.group_mut(id) {
                    g.visible = !g.visible;
                }
            } else if let Some(l) = app.project.layer_mut(id) {
                l.visible = !l.visible;
            }
        }
        TreeOp::ToggleCollapse(id) => {
            if let Some(g) = app.project.group_mut(id) {
                g.collapsed = !g.collapsed;
            }
        }
        TreeOp::StartDrag(item) => app.panel_drag = Some(item),
        TreeOp::StartRename(id) => {
            let name = app
                .project
                .group(id)
                .map(|g| g.name.clone())
                .or_else(|| app.project.layer(id).map(|l| l.name.clone()))
                .unwrap_or_default();
            app.renaming = Some(id);
            app.rename_buf = name;
            app.rename_focus = true;
        }
        TreeOp::CommitRename(id) => {
            let b = app.rename_buf.clone();
            app.rename(id, b);
            app.renaming = None;
        }
        TreeOp::CancelRename => app.renaming = None,
        TreeOp::Duplicate(id) => app.duplicate(id),
        TreeOp::Delete(id) => {
            app.begin_edit();
            app.project.delete(id);
            if app.selection.map(|s| s.id()) == Some(id) {
                app.selection = None;
            }
        }
        TreeOp::Ungroup(id) => app.ungroup(id),
        TreeOp::Tag(id) => {
            let choice = app.project.layer(id).and_then(|l| l.tag);
            app.selection = Some(Selection::Layer(id));
            app.tag_dialog = Some(TagDialog { layer_id: id, choice });
        }
    }
}

// ---- slots ----------------------------------------------------------------

enum SlotOp {
    Select(u64),
    ToggleVisible(usize),
    Delete(usize),
}

fn slots_list(app: &mut App, ui: &mut egui::Ui) {
    let sel = app.selection;
    let mut ops: Vec<SlotOp> = Vec::new();
    for i in 0..app.project.slots.len() {
        let id = app.project.slots[i].id;
        let is_sel = matches!(sel, Some(Selection::Slot(s)) if s == id);
        let role = app.project.slots[i].role;
        let cells = app.project.slots[i].grid.cols.max(1) * app.project.slots[i].grid.rows.max(1);
        ui.horizontal(|ui| {
            if eye_button(ui, app.project.slots[i].visible) {
                ops.push(SlotOp::ToggleVisible(i));
            }
            let label = format!("{}  ×{}", role.label(), cells);
            if ui.selectable_label(is_sel, label).clicked() {
                ops.push(SlotOp::Select(id));
            }
            ui.with_layout(rtl(), |ui| {
                if ui.small_button(icons::TRASH).on_hover_text("Delete").clicked() {
                    ops.push(SlotOp::Delete(i));
                }
            });
        });
    }
    if app.project.slots.is_empty() {
        ui.label(RichText::new("No slots yet — drag “New Slot” or click ＋ Slot.").weak().small());
    }
    for op in ops {
        match op {
            SlotOp::Select(id) => app.selection = Some(Selection::Slot(id)),
            SlotOp::ToggleVisible(i) => {
                if i < app.project.slots.len() {
                    app.begin_edit();
                    app.project.slots[i].visible = !app.project.slots[i].visible;
                }
            }
            SlotOp::Delete(i) => {
                if i < app.project.slots.len() {
                    app.begin_edit();
                    let id = app.project.slots[i].id;
                    app.project.slots.remove(i);
                    if matches!(app.selection, Some(Selection::Slot(s)) if s == id) {
                        app.selection = None;
                    }
                }
            }
        }
    }
}

// ---- inspector ------------------------------------------------------------

fn inspector(app: &mut App, ui: &mut egui::Ui) {
    match app.selection {
        Some(Selection::Layer(id)) => {
            if app.project.layer(id).is_some() {
                layer_inspector(app, ui, id);
            } else {
                ui.label("Layer not found.");
            }
        }
        Some(Selection::Group(id)) => {
            if app.project.group(id).is_some() {
                group_inspector(app, ui, id);
            } else {
                ui.label("Group not found.");
            }
        }
        Some(Selection::Slot(_)) => match app.selected_slot_idx() {
            Some(i) => slot_inspector(app, ui, i),
            None => {
                ui.label("Slot not found.");
            }
        },
        None => {
            ui.label(RichText::new("Select a layer, group, or slot to edit it.").weak());
        }
    }
}

fn drag2(ui: &mut egui::Ui, label: &str, x: &mut i32, y: &mut i32) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(x).speed(1.0));
        ui.add(egui::DragValue::new(y).speed(1.0));
    });
}

fn layer_inspector(app: &mut App, ui: &mut egui::Ui, id: u64) {
    app.begin_edit();
    let Some(l) = app.project.layer_mut(id) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut l.name);
    });
    drag2(ui, "pos x/y", &mut l.rect.x, &mut l.rect.y);
    drag2(ui, "size w/h", &mut l.rect.w, &mut l.rect.h);
    l.rect.w = l.rect.w.max(1);
    l.rect.h = l.rect.h.max(1);
    ui.horizontal(|ui| {
        ui.checkbox(&mut l.flip_h, "Flip H");
        ui.checkbox(&mut l.flip_v, "Flip V");
    });
    ui.add(egui::Slider::new(&mut l.rotation, 0..=359).text("rotation").suffix("°"));

    let mut mode = match l.fit {
        LayerFit::Stretch => 0usize,
        LayerFit::Tile => 1,
        LayerFit::NineSlice { .. } => 2,
    };
    egui::ComboBox::from_id_salt(("fit", id))
        .selected_text(["Stretch", "Tile", "Nine-slice"][mode])
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut mode, 0, "Stretch");
            ui.selectable_value(&mut mode, 1, "Tile");
            ui.selectable_value(&mut mode, 2, "Nine-slice");
        });
    let cur = l.fit;
    l.fit = match mode {
        0 => LayerFit::Stretch,
        1 => LayerFit::Tile,
        _ => match cur {
            LayerFit::NineSlice { .. } => cur,
            _ => LayerFit::NineSlice { l: 4, r: 4, t: 4, b: 4 },
        },
    };
    if let LayerFit::NineSlice { l: il, r: ir, t: it, b: ib } = &mut l.fit {
        ui.horizontal(|ui| {
            ui.label("insets L/R/T/B");
            ui.add(egui::DragValue::new(il).speed(0.3));
            ui.add(egui::DragValue::new(ir).speed(0.3));
            ui.add(egui::DragValue::new(it).speed(0.3));
            ui.add(egui::DragValue::new(ib).speed(0.3));
        });
    }
    ui.add(egui::Slider::new(&mut l.opacity, 0.0..=1.0).text("opacity"));
}

fn group_inspector(app: &mut App, ui: &mut egui::Ui, id: u64) {
    app.begin_edit();
    let Some(g) = app.project.group_mut(id) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Group");
        ui.text_edit_singleline(&mut g.name);
    });
    ui.checkbox(&mut g.visible, "Visible");
    ui.label(RichText::new(format!("{} layer(s)", g.children.len())).weak());
    ui.label(RichText::new("Select on canvas to drag the whole group.").weak().small());
}

fn slot_inspector(app: &mut App, ui: &mut egui::Ui, i: usize) {
    app.begin_edit();
    let mut role = app.project.slots[i].role;
    egui::ComboBox::from_id_salt(("slot_role", i)).selected_text(role.label()).show_ui(ui, |ui| {
        for r in SlotRole::ALL {
            ui.selectable_value(&mut role, r, r.label());
        }
    });
    if role != app.project.slots[i].role {
        app.project.slots[i].role = role;
        let t = role.tint();
        let a = app.project.slots[i].color[3];
        app.project.slots[i].color = [t[0], t[1], t[2], a];
    }

    {
        let s = &mut app.project.slots[i];
        drag2(ui, "pos x/y", &mut s.rect.x, &mut s.rect.y);
        drag2(ui, "cell w/h", &mut s.rect.w, &mut s.rect.h);
        s.rect.w = s.rect.w.max(1);
        s.rect.h = s.rect.h.max(1);
        ui.horizontal(|ui| {
            ui.label("grid c/r");
            ui.add(egui::DragValue::new(&mut s.grid.cols).speed(0.1));
            ui.add(egui::DragValue::new(&mut s.grid.rows).speed(0.1));
        });
        s.grid.cols = s.grid.cols.max(1);
        s.grid.rows = s.grid.rows.max(1);
        drag2(ui, "pitch x/y", &mut s.grid.pitch_x, &mut s.grid.pitch_y);
    }

    let s = &mut app.project.slots[i];
    let mut c = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], s.color[3]);
    ui.horizontal(|ui| {
        ui.label("box color");
        if ui.color_edit_button_srgba(&mut c).changed() {
            s.color = [c.r(), c.g(), c.b(), c.a()];
        }
    });
    ui.checkbox(&mut s.paint_frame, "Bake slot frame into PNG");
}

/// Per-GUI hover highlight: pick the graphic, its margin (how far it extends
/// beyond the slot so the slot frame sits inside it), fit mode and opacity. Baked
/// to a sibling PNG + the manifest so the game can draw it on hover.
fn hover_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.label(RichText::new("drawn over a hovered slot, inflated by margin so the slot frame sits inside").weak().small());

    if app.project.hover.is_none() {
        if ui.button(format!("{} Add hover highlight", icons::PLUS)).clicked() {
            app.begin_edit();
            app.project.hover = Some(Hover {
                asset: AssetSpec::Builtin { key: "highlight".to_string() },
                margin: 4,
                fit: LayerFit::NineSlice { l: 4, r: 4, t: 4, b: 4 },
                opacity: 1.0,
            });
        }
        return;
    }

    app.begin_edit();
    // Graphic picker: a combo over every palette asset (builtins + imported).
    let choices: Vec<(String, AssetSpec)> =
        app.assets.entries.iter().map(|e| (e.label.clone(), e.spec.clone())).collect();
    let cur = app.project.hover.as_ref().map(|h| h.asset.clone());
    let cur_label = cur
        .as_ref()
        .and_then(|s| choices.iter().find(|(_, sp)| sp == s))
        .map(|(l, _)| l.clone())
        .unwrap_or_else(|| "(missing)".to_string());
    ui.horizontal(|ui| {
        ui.label("graphic");
        egui::ComboBox::from_id_salt("hover_asset").selected_text(cur_label).show_ui(ui, |ui| {
            for (label, spec) in &choices {
                let selected = cur.as_ref() == Some(spec);
                if ui.selectable_label(selected, label).clicked() {
                    if let Some(h) = &mut app.project.hover {
                        h.asset = spec.clone();
                    }
                }
            }
        });
    });

    if let Some(h) = &mut app.project.hover {
        ui.horizontal(|ui| {
            ui.label("margin (px beyond slot)");
            ui.add(egui::DragValue::new(&mut h.margin).speed(0.5));
        });
        h.margin = h.margin.max(0);

        let mut mode = match h.fit {
            LayerFit::Stretch => 0usize,
            LayerFit::Tile => 1,
            LayerFit::NineSlice { .. } => 2,
        };
        egui::ComboBox::from_id_salt("hover_fit")
            .selected_text(["Stretch", "Tile", "Nine-slice"][mode])
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, 0, "Stretch");
                ui.selectable_value(&mut mode, 1, "Tile");
                ui.selectable_value(&mut mode, 2, "Nine-slice");
            });
        let cur_fit = h.fit;
        h.fit = match mode {
            0 => LayerFit::Stretch,
            1 => LayerFit::Tile,
            _ => match cur_fit {
                LayerFit::NineSlice { .. } => cur_fit,
                _ => LayerFit::NineSlice { l: 4, r: 4, t: 4, b: 4 },
            },
        };
        if let LayerFit::NineSlice { l: il, r: ir, t: it, b: ib } = &mut h.fit {
            ui.horizontal(|ui| {
                ui.label("insets L/R/T/B");
                ui.add(egui::DragValue::new(il).speed(0.3));
                ui.add(egui::DragValue::new(ir).speed(0.3));
                ui.add(egui::DragValue::new(it).speed(0.3));
                ui.add(egui::DragValue::new(ib).speed(0.3));
            });
        }
        ui.add(egui::Slider::new(&mut h.opacity, 0.0..=1.0).text("opacity"));
    }

    if ui.button(format!("{} Remove", icons::TRASH)).clicked() {
        app.project.hover = None;
    }
    ui.label(RichText::new("Select a slot to preview the highlight on it.").weak().small());
}

// ===========================================================================
// Right panel: asset palette
// ===========================================================================

struct PaletteItem {
    spec: AssetSpec,
    label: String,
    size: [usize; 2],
    fit: LayerFit,
    cover: bool,
    kind: crate::assets::AssetKind,
    tex: egui::TextureId,
}

pub fn right_panel(app: &mut App, ctx: &egui::Context) {
    egui::SidePanel::right("assets").resizable(true).default_width(232.0).show(ctx, |ui| {
        ui.add_space(4.0);
        ui.heading("Assets");
        if ui.button("Load PNG…").clicked() {
            load_png(app, ctx);
        }
        ui.label(RichText::new("Click to add at center, or drag onto the canvas.").weak().small());
        ui.separator();

        ui.label(RichText::new("Tools").strong());
        let slot_resp = tool_button(ui, &format!("{} New Slot", icons::PLUS), Color32::from_rgba_unmultiplied(120, 160, 220, 150));
        if slot_resp.drag_started() {
            app.drag = Some(DragPayload::Slot);
        }
        if slot_resp.clicked() {
            add_slot_center(app);
        }
        ui.separator();

        let items: Vec<PaletteItem> = app
            .assets
            .entries
            .iter()
            .filter_map(|e| {
                app.assets.get(&e.spec).map(|d| PaletteItem {
                    spec: e.spec.clone(),
                    label: e.label.clone(),
                    size: d.size,
                    fit: e.default_fit,
                    cover: e.cover,
                    kind: e.kind,
                    tex: d.tex.id(),
                })
            })
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for kind in crate::assets::AssetKind::ORDER {
                let group: Vec<&PaletteItem> = items.iter().filter(|it| it.kind == kind).collect();
                if group.is_empty() {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(RichText::new(kind.label()).strong());
                ui.horizontal_wrapped(|ui| {
                    for it in group {
                        let resp = asset_thumb(ui, it.tex, &it.label);
                        if resp.drag_started() {
                            app.drag = Some(DragPayload::Asset { spec: it.spec.clone(), size: it.size, default_fit: it.fit, cover: it.cover });
                        }
                        if resp.clicked() {
                            add_layer_center(app, it.spec.clone(), it.size, it.fit, it.cover);
                        }
                    }
                });
            }
        });
    });
}

fn asset_thumb(ui: &mut egui::Ui, tex: egui::TextureId, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_gray(52));
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    p.image(tex, rect.shrink(4.0), uv, Color32::WHITE);
    let hot = resp.hovered();
    p.rect_stroke(rect, 3.0, Stroke::new(if hot { 2.0 } else { 1.0 }, Color32::from_gray(if hot { 210 } else { 90 })));
    resp.on_hover_text(label)
}

fn tool_button(ui: &mut egui::Ui, label: &str, fill: Color32) -> egui::Response {
    let w = ui.available_width().min(160.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, fill);
    p.rect_stroke(rect, 4.0, Stroke::new(1.0, Color32::WHITE));
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(13.0), Color32::WHITE);
    resp
}

// ---- helpers --------------------------------------------------------------

fn load_png(app: &mut App, ctx: &egui::Context) {
    if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).pick_file() {
        match app.assets.load_file(ctx, &path) {
            Ok(_) => app.status = format!("Imported {}", path.display()),
            Err(e) => app.status = format!("Import failed: {e}"),
        }
    }
}

fn add_layer_center(app: &mut App, spec: AssetSpec, size: [usize; 2], fit: LayerFit, cover: bool) {
    app.begin_edit();
    let name = app.assets.entry(&spec).map(|e| e.label.clone()).unwrap_or_else(|| "Layer".to_string());
    let id = app.alloc_id();
    let rect = if cover {
        RectF::new(0, 0, app.project.canvas.w as i32, app.project.canvas.h as i32)
    } else {
        let (cx, cy) = (app.project.canvas.w as i32 / 2, app.project.canvas.h as i32 / 2);
        RectF::new(cx - size[0] as i32 / 2, cy - size[1] as i32 / 2, size[0] as i32, size[1] as i32)
    };
    app.project.nodes.push(Node::Layer(Layer {
        id,
        name,
        asset: spec,
        rect,
        fit,
        opacity: 1.0,
        visible: true,
        flip_h: false,
        flip_v: false,
        rotation: 0,
        tag: None,
    }));
    app.selection = Some(Selection::Layer(id));
}

fn add_slot_center(app: &mut App) {
    app.begin_edit();
    let (cx, cy) = (app.project.canvas.w as i32 / 2, app.project.canvas.h as i32 / 2);
    let id = app.alloc_id();
    let role = SlotRole::Generic;
    let t = role.tint();
    app.project.slots.push(Slot {
        id,
        role,
        rect: RectF::new(cx - 9, cy - 9, 18, 18),
        grid: Grid::default(),
        color: [t[0], t[1], t[2], 130],
        paint_frame: true,
        visible: true,
    });
    app.selection = Some(Selection::Slot(id));
}
