//! The central editing canvas: draws the composited layers + slot overlays, and
//! handles selection, drag-to-move, resize, group move, palette drop, and zoom.
//!
//! Move/resize run through a drag-gesture state machine (`App::active_drag`):
//! each frame we re-derive the rect(s) as `start + processed_delta`, where the
//! delta passes through axis-lock (Shift) and snap (grid, unless Ctrl), then
//! rounds to whole pixels. A selected *group* drags all its children rigidly.

use crate::app::{App, Axis, DragMode, DragMove, DragPayload, Selection};
use crate::bake::layer_regions;
use crate::model::{AssetSpec, Grid, Layer, LayerFit, Node, RectF, Slot, SlotRole};
use eframe::egui::{self, Color32, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, Vec2};

enum Act {
    Select(Selection),
    Deselect,
    StartDrag { sel: Selection, mode: DragMode, start_rect: RectF },
    StartDragGroup { gid: u64, start_bbox: RectF },
    DrillInto { gid: u64, x: i32, y: i32 },
    AddLayer { spec: AssetSpec, size: [usize; 2], fit: LayerFit, cover: bool, base: Pos2 },
    AddSlot { base: Pos2 },
}

pub fn show_canvas(app: &mut App, ui: &mut egui::Ui) {
    let avail = ui.available_rect_before_wrap();
    let painter = ui.painter_at(avail);
    painter.rect_filled(avail, 0.0, Color32::from_gray(32));

    let ctx = ui.ctx().clone();
    let pointer = ctx.input(|i| i.pointer.hover_pos());

    // Right-button drag pans the view.
    if ctx.input(|i| i.pointer.secondary_pressed()) && pointer.map_or(false, |p| avail.contains(p)) {
        app.panning = true;
    }
    if !ctx.input(|i| i.pointer.secondary_down()) {
        app.panning = false;
    }
    if app.panning {
        app.view.pan += ctx.input(|i| i.pointer.delta());
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    let cw = app.project.canvas.w.max(1) as f32;
    let ch = app.project.canvas.h.max(1) as f32;
    let margin = 28.0;
    let fit = ((avail.width() - margin) / cw).min((avail.height() - margin) / ch).max(0.02);
    let scale = (fit * app.view.zoom).max(0.02);
    let size = egui::vec2(cw * scale, ch * scale);
    let origin = avail.center() - size * 0.5 + app.view.pan;
    let canvas_rect = Rect::from_min_size(origin, size);
    let to_screen_rect = |r: RectF| {
        Rect::from_min_size(
            egui::pos2(origin.x + r.x as f32 * scale, origin.y + r.y as f32 * scale),
            egui::vec2(r.w as f32 * scale, r.h as f32 * scale),
        )
    };

    draw_checker(&painter, canvas_rect);
    painter.rect_stroke(canvas_rect, 0.0, Stroke::new(1.0, Color32::from_gray(90)));

    let mut acts: Vec<Act> = Vec::new();

    let bg = ui.interact(avail, Id::new("canvas_bg"), Sense::click());
    if bg.clicked() {
        acts.push(Act::Deselect);
    }

    // Layers (back-to-front), respecting group visibility.
    let flats = app.project.flat_layers();
    for fl in &flats {
        if fl.effective_visible {
            draw_layer(&painter, app, fl.layer, origin, scale);
        }
        // The interact rect is the rotated quad's bounding box, but a hit only
        // counts if the pointer is actually inside the rotated quad — so the
        // hitbox follows rotation.
        let corners = layer_corners_screen(fl.layer, origin, scale);
        let resp = ui.interact(aabb_of(&corners), Id::new(("layer", fl.layer.id)), Sense::click_and_drag());
        let inside = pointer.map_or(false, |p| point_in_layer(fl.layer, p, origin, scale));
        if resp.drag_started_by(egui::PointerButton::Primary) && inside {
            acts.push(Act::StartDrag { sel: Selection::Layer(fl.layer.id), mode: DragMode::Move, start_rect: fl.layer.rect });
        } else if resp.clicked() && inside {
            acts.push(Act::Select(Selection::Layer(fl.layer.id)));
        }
    }
    drop(flats);

    // Slots on top.
    if app.show_slots {
        for slot in app.project.slots.iter() {
            if !slot.visible {
                continue;
            }
            let selected = matches!(app.selection, Some(Selection::Slot(id)) if id == slot.id);
            draw_slot(&painter, slot, origin, scale, selected);
            let scr = to_screen_rect(slot_bounds(slot));
            let resp = ui.interact(scr, Id::new(("slot", slot.id)), Sense::click_and_drag());
            if resp.drag_started_by(egui::PointerButton::Primary) {
                acts.push(Act::StartDrag { sel: Selection::Slot(slot.id), mode: DragMode::Move, start_rect: slot.rect });
            } else if resp.clicked() {
                acts.push(Act::Select(Selection::Slot(slot.id)));
            }
        }
    }

    // Hover-highlight preview: draw the highlight graphic (inflated by `margin`)
    // on the selected slot's first cell and on the slot cell under the cursor,
    // mirroring how the game will draw it on hover.
    if let Some(hover) = app.project.hover.clone() {
        let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
        let mut cells: Vec<RectF> = Vec::new();
        if let Some(Selection::Slot(id)) = app.selection {
            if let Some(c) = app.project.slots.iter().find(|s| s.id == id).and_then(|s| s.cells().into_iter().next()) {
                cells.push(c);
            }
        }
        if let Some(p) = pointer {
            let bx = ((p.x - origin.x) / scale).floor() as i32;
            let by = ((p.y - origin.y) / scale).floor() as i32;
            if let Some(c) = app.project.slots.iter().filter(|s| s.visible).flat_map(|s| s.cells()).find(|c| c.contains(bx, by)) {
                cells.push(c);
            }
        }
        for cell in cells {
            draw_hover_preview(&painter, app, &hover, cell, origin, scale);
        }
    }

    // Selection manipulation (registered last = on top).
    match app.selection {
        Some(Selection::Group(gid)) => {
            if let Some(bb) = app.project.group_bounds(gid) {
                let scr = to_screen_rect(bb);
                painter.rect_stroke(scr, 0.0, Stroke::new(2.0, Color32::from_rgb(250, 220, 90)));
                let resp = ui.interact(scr, Id::new(("group_drag", gid)), Sense::click_and_drag());
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    acts.push(Act::StartDragGroup { gid, start_bbox: bb });
                } else if resp.clicked() {
                    if let Some(p) = pointer {
                        let x = ((p.x - origin.x) / scale).floor() as i32;
                        let y = ((p.y - origin.y) / scale).floor() as i32;
                        acts.push(Act::DrillInto { gid, x, y });
                    }
                }
            }
        }
        Some(Selection::Layer(id)) => {
            if let Some(l) = app.project.layer(id) {
                // Selection outline + resize handle follow the rotation.
                let corners = layer_corners_screen(l, origin, scale);
                painter.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(1.5, Color32::from_rgb(250, 220, 90))));
                let handle = Rect::from_center_size(corners[2], egui::vec2(11.0, 11.0));
                painter.rect_filled(handle, 2.0, Color32::from_rgb(250, 220, 90));
                painter.rect_stroke(handle, 2.0, Stroke::new(1.0, Color32::BLACK));
                let resp = ui.interact(handle, Id::new("resize_handle"), Sense::click_and_drag());
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    acts.push(Act::StartDrag { sel: Selection::Layer(id), mode: DragMode::Resize, start_rect: l.rect });
                }
            }
        }
        Some(Selection::Slot(_)) => {
            if let Some((sel, own_rect, bounds)) = selection_target(app) {
                let scr = to_screen_rect(bounds);
                let handle = Rect::from_center_size(scr.right_bottom(), egui::vec2(11.0, 11.0));
                painter.rect_filled(handle, 2.0, Color32::from_rgb(250, 220, 90));
                painter.rect_stroke(handle, 2.0, Stroke::new(1.0, Color32::BLACK));
                let resp = ui.interact(handle, Id::new("resize_handle"), Sense::click_and_drag());
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    acts.push(Act::StartDrag { sel, mode: DragMode::Resize, start_rect: own_rect });
                }
            }
        }
        None => {}
    }

    // Palette drag: ghost + drop.
    let mut clear_drag = false;
    if let Some(payload) = app.drag.as_ref() {
        if let Some(p) = pointer {
            let gp = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("drag_ghost")));
            match payload {
                DragPayload::Asset { spec, .. } => {
                    if let Some(d) = app.assets.get(spec) {
                        let r = Rect::from_center_size(p, egui::vec2(44.0, 44.0));
                        gp.image(d.tex.id(), r, full_uv(), Color32::from_white_alpha(190));
                    }
                }
                DragPayload::Slot => {
                    let r = Rect::from_center_size(p, egui::vec2(20.0, 20.0));
                    gp.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(120, 160, 220, 140));
                    gp.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE));
                }
            }
        }
        if ctx.input(|i| i.pointer.any_released()) {
            if let Some(p) = pointer {
                if avail.contains(p) {
                    let base = egui::pos2((p.x - origin.x) / scale, (p.y - origin.y) / scale);
                    match payload {
                        DragPayload::Asset { spec, size, default_fit, cover } => acts.push(Act::AddLayer {
                            spec: spec.clone(),
                            size: *size,
                            fit: *default_fit,
                            cover: *cover,
                            base,
                        }),
                        DragPayload::Slot => acts.push(Act::AddSlot { base }),
                    }
                }
            }
            clear_drag = true;
        }
    }
    if clear_drag {
        app.drag = None;
    }

    apply_actions(app, acts);
    update_drag_gesture(app, &ctx, scale);

    if pointer.map_or(false, |p| avail.contains(p)) {
        let dy = ctx.input(|i| i.raw_scroll_delta.y);
        if dy.abs() > 0.0 {
            app.view.zoom = (app.view.zoom * (1.0 + dy * 0.0015)).clamp(0.1, 8.0);
        }
    }
}

fn apply_actions(app: &mut App, acts: Vec<Act>) {
    for act in acts {
        match act {
            Act::Select(s) => app.selection = Some(s),
            Act::Deselect => app.selection = None,
            Act::StartDrag { sel, mode, start_rect } => {
                app.begin_edit();
                app.selection = Some(sel);
                app.active_drag = Some(DragMove {
                    sel,
                    mode,
                    start_rect,
                    group_children: Vec::new(),
                    raw_total: Vec2::ZERO,
                    shift_anchor: None,
                    axis: None,
                });
            }
            Act::StartDragGroup { gid, start_bbox } => {
                app.begin_edit();
                app.selection = Some(Selection::Group(gid));
                let children = app.project.group_child_rects(gid);
                app.active_drag = Some(DragMove {
                    sel: Selection::Group(gid),
                    mode: DragMode::Move,
                    start_rect: start_bbox,
                    group_children: children,
                    raw_total: Vec2::ZERO,
                    shift_anchor: None,
                    axis: None,
                });
            }
            Act::DrillInto { gid, x, y } => {
                // Topmost descendant layer of the group under the pointer.
                let sub = app.project.subtree_ids(gid);
                let mut hit = None;
                for fl in app.project.flat_layers() {
                    if sub.contains(&fl.layer.id) && fl.layer.rect.contains(x, y) {
                        hit = Some(fl.layer.id);
                    }
                }
                if let Some(id) = hit {
                    app.selection = Some(Selection::Layer(id));
                }
            }
            Act::AddLayer { spec, size, fit, cover, base } => {
                app.begin_edit();
                let name = app.assets.entry(&spec).map(|e| e.label.clone()).unwrap_or_else(|| "Layer".to_string());
                let id = app.alloc_id();
                let rect = if cover {
                    RectF::new(0, 0, app.project.canvas.w as i32, app.project.canvas.h as i32)
                } else {
                    let (w, h) = (size[0] as i32, size[1] as i32);
                    RectF::new(base.x.round() as i32 - w / 2, base.y.round() as i32 - h / 2, w, h)
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
            Act::AddSlot { base } => {
                app.begin_edit();
                let id = app.alloc_id();
                let role = SlotRole::Generic;
                let t = role.tint();
                app.project.slots.push(Slot {
                    id,
                    role,
                    rect: RectF::new(base.x.round() as i32 - 9, base.y.round() as i32 - 9, 18, 18),
                    grid: Grid::default(),
                    color: [t[0], t[1], t[2], 130],
                    paint_frame: true,
                    visible: true,
                });
                app.selection = Some(Selection::Slot(id));
            }
        }
    }
}

fn update_drag_gesture(app: &mut App, ctx: &egui::Context, scale: f32) {
    let Some(mut drag) = app.active_drag.take() else {
        return;
    };
    // Move/resize is a primary-button gesture; ends when primary is released
    // (and never runs during a right-button pan).
    if !ctx.input(|i| i.pointer.primary_down()) {
        return;
    }

    let frame = ctx.input(|i| i.pointer.delta());
    drag.raw_total += egui::vec2(frame.x / scale, frame.y / scale);

    if ctx.input(|i| i.modifiers.shift) {
        let anchor = *drag.shift_anchor.get_or_insert(drag.raw_total);
        if drag.axis.is_none() {
            let d = drag.raw_total - anchor;
            if d.x.abs().max(d.y.abs()) > 0.5 {
                drag.axis = Some(if d.x.abs() >= d.y.abs() { Axis::Horizontal } else { Axis::Vertical });
            }
        }
    } else {
        drag.shift_anchor = None;
        drag.axis = None;
    }

    let eff = match (drag.shift_anchor, drag.axis) {
        (Some(anchor), Some(axis)) => lock_axis(drag.raw_total, anchor, axis),
        _ => drag.raw_total,
    };

    let ctrl = ctx.input(|i| i.modifiers.ctrl);
    let snap = if app.snap_enabled && !ctrl { Some(app.snap_step.max(1)) } else { None };

    apply_drag(app, &drag, eff, snap);
    app.active_drag = Some(drag);
}

fn snap_to(v: i32, step: i32) -> i32 {
    ((v as f32 / step as f32).round() as i32) * step
}

fn lock_axis(raw_total: Vec2, anchor: Vec2, axis: Axis) -> Vec2 {
    let mut eff = raw_total;
    match axis {
        Axis::Horizontal => eff.y = anchor.y,
        Axis::Vertical => eff.x = anchor.x,
    }
    eff
}

fn resolve_move(start: RectF, eff: Vec2, snap: Option<i32>) -> (i32, i32) {
    let mut x = (start.x as f32 + eff.x).round() as i32;
    let mut y = (start.y as f32 + eff.y).round() as i32;
    if let Some(s) = snap {
        x = snap_to(x, s);
        y = snap_to(y, s);
    }
    (x, y)
}

fn resolve_resize(start: RectF, eff: Vec2, snap: Option<i32>) -> (i32, i32) {
    let mut w = (start.w as f32 + eff.x).round() as i32;
    let mut h = (start.h as f32 + eff.y).round() as i32;
    if let Some(s) = snap {
        w = snap_to(w, s).max(s);
        h = snap_to(h, s).max(s);
    }
    (w.max(1), h.max(1))
}

/// New top-left so that the rotated top-left corner (opposite the bottom-right
/// resize handle) stays put when the size changes. Rotation is about the centre,
/// so resizing must shift `(x,y)` to keep the pivot consistent. For 0° this just
/// returns the original `(x,y)`.
fn resize_anchor_topleft(start: RectF, rot_deg: i32, w: i32, h: i32) -> (i32, i32) {
    let a = (rot_deg as f32).to_radians();
    let (s, c) = (a.sin(), a.cos());
    let rot = |dx: f32, dy: f32| (c * dx - s * dy, s * dx + c * dy);
    let (cx0, cy0) = (start.x as f32 + start.w as f32 * 0.5, start.y as f32 + start.h as f32 * 0.5);
    let (ax, ay) = {
        let (rx, ry) = rot(-start.w as f32 * 0.5, -start.h as f32 * 0.5);
        (cx0 + rx, cy0 + ry)
    };
    let (rx, ry) = rot(w as f32 * 0.5, h as f32 * 0.5);
    let (cx1, cy1) = (ax + rx, ay + ry);
    ((cx1 - w as f32 * 0.5).round() as i32, (cy1 - h as f32 * 0.5).round() as i32)
}

fn apply_drag(app: &mut App, drag: &DragMove, eff: Vec2, snap: Option<i32>) {
    let start = drag.start_rect;
    match drag.mode {
        DragMode::Move => {
            let (nx, ny) = resolve_move(start, eff, snap);
            if drag.group_children.is_empty() {
                set_selected_pos(app, drag.sel, nx, ny);
            } else {
                let (dx, dy) = (nx - start.x, ny - start.y);
                for (cid, cr) in &drag.group_children {
                    if let Some(l) = app.project.layer_mut(*cid) {
                        l.rect.x = cr.x + dx;
                        l.rect.y = cr.y + dy;
                    }
                }
            }
        }
        DragMode::Resize => match drag.sel {
            Selection::Layer(id) => {
                let rot = app.project.layer(id).map(|l| l.rotation).unwrap_or(0);
                // Map the screen-space drag delta into the layer's local axes so
                // the handle resizes along its own edges.
                let e = if rot.rem_euclid(360) != 0 {
                    let a = (rot as f32).to_radians();
                    let (s, c) = (a.sin(), a.cos());
                    egui::vec2(c * eff.x + s * eff.y, -s * eff.x + c * eff.y)
                } else {
                    eff
                };
                let (w, h) = resolve_resize(start, e, snap);
                let (x, y) = resize_anchor_topleft(start, rot, w, h);
                if let Some(l) = app.project.layer_mut(id) {
                    l.rect = RectF::new(x, y, w, h);
                }
            }
            Selection::Slot(_) => {
                let (w, h) = resolve_resize(start, eff, snap);
                set_selected_size(app, drag.sel, w, h);
            }
            Selection::Group(_) => {}
        },
    }
}

fn set_selected_pos(app: &mut App, sel: Selection, x: i32, y: i32) {
    match sel {
        Selection::Layer(id) => {
            if let Some(l) = app.project.layer_mut(id) {
                l.rect.x = x;
                l.rect.y = y;
            }
        }
        Selection::Slot(id) => {
            if let Some(s) = app.project.slots.iter_mut().find(|s| s.id == id) {
                s.rect.x = x;
                s.rect.y = y;
            }
        }
        Selection::Group(_) => {}
    }
}

fn set_selected_size(app: &mut App, sel: Selection, w: i32, h: i32) {
    match sel {
        Selection::Layer(id) => {
            if let Some(l) = app.project.layer_mut(id) {
                l.rect.w = w;
                l.rect.h = h;
            }
        }
        Selection::Slot(id) => {
            if let Some(s) = app.project.slots.iter_mut().find(|s| s.id == id) {
                s.rect.w = w;
                s.rect.h = h;
            }
        }
        Selection::Group(_) => {}
    }
}

// ---- drawing helpers ------------------------------------------------------

fn full_uv() -> Rect {
    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}

/// Preview the hover highlight over one slot cell, inflated by `margin` on every
/// side (so the slot frame sits inside it) — the same geometry the game uses.
fn draw_hover_preview(painter: &egui::Painter, app: &App, hover: &crate::model::Hover, cell: RectF, origin: Pos2, scale: f32) {
    let Some(data) = app.assets.get(&hover.asset) else {
        return;
    };
    let m = hover.margin;
    let rect = RectF::new(cell.x - m, cell.y - m, cell.w + 2 * m, cell.h + 2 * m);
    let tint = Color32::from_white_alpha((hover.opacity.clamp(0.0, 1.0) * 255.0) as u8);
    let to_scr = |x: f32, y: f32| egui::pos2(origin.x + x * scale, origin.y + y * scale);
    for reg in layer_regions(hover.fit, data.size[0], data.size[1], rect.x, rect.y, rect.w, rect.h, false, false) {
        let scr = Rect::from_min_max(to_scr(reg.dst[0], reg.dst[1]), to_scr(reg.dst[0] + reg.dst[2], reg.dst[1] + reg.dst[3]));
        let uv = Rect::from_min_max(egui::pos2(reg.uv[0], reg.uv[1]), egui::pos2(reg.uv[2], reg.uv[3]));
        painter.image(data.tex.id(), scr, uv, tint);
    }
}

fn draw_layer(painter: &egui::Painter, app: &App, layer: &Layer, origin: Pos2, scale: f32) {
    let Some(data) = app.assets.get(&layer.asset) else {
        return;
    };
    let tint = Color32::from_white_alpha((layer.opacity.clamp(0.0, 1.0) * 255.0) as u8);
    let regs = layer_regions(
        layer.fit,
        data.size[0],
        data.size[1],
        layer.rect.x,
        layer.rect.y,
        layer.rect.w,
        layer.rect.h,
        layer.flip_h,
        layer.flip_v,
    );

    let to_scr = |x: f32, y: f32| egui::pos2(origin.x + x * scale, origin.y + y * scale);

    if layer.rotation.rem_euclid(360) == 0 {
        for reg in regs {
            let scr = Rect::from_min_max(to_scr(reg.dst[0], reg.dst[1]), to_scr(reg.dst[0] + reg.dst[2], reg.dst[1] + reg.dst[3]));
            let uv = Rect::from_min_max(egui::pos2(reg.uv[0], reg.uv[1]), egui::pos2(reg.uv[2], reg.uv[3]));
            painter.image(data.tex.id(), scr, uv, tint);
        }
        return;
    }

    let ang = (layer.rotation as f32).to_radians();
    let (sin, cos) = (ang.sin(), ang.cos());
    let center = to_scr(layer.rect.x as f32 + layer.rect.w as f32 * 0.5, layer.rect.y as f32 + layer.rect.h as f32 * 0.5);
    let rotate = |p: Pos2| {
        let (dx, dy) = (p.x - center.x, p.y - center.y);
        egui::pos2(center.x + cos * dx - sin * dy, center.y + sin * dx + cos * dy)
    };
    let mut mesh = egui::Mesh::with_texture(data.tex.id());
    for reg in regs {
        let (x0, y0) = (reg.dst[0], reg.dst[1]);
        let (x1, y1) = (x0 + reg.dst[2], y0 + reg.dst[3]);
        let corners = [to_scr(x0, y0), to_scr(x1, y0), to_scr(x1, y1), to_scr(x0, y1)];
        let uvs = [
            egui::pos2(reg.uv[0], reg.uv[1]),
            egui::pos2(reg.uv[2], reg.uv[1]),
            egui::pos2(reg.uv[2], reg.uv[3]),
            egui::pos2(reg.uv[0], reg.uv[3]),
        ];
        let base = mesh.vertices.len() as u32;
        for k in 0..4 {
            mesh.vertices.push(egui::epaint::Vertex { pos: rotate(corners[k]), uv: uvs[k], color: tint });
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn draw_slot(painter: &egui::Painter, slot: &Slot, origin: Pos2, scale: f32, selected: bool) {
    let fill = Color32::from_rgba_unmultiplied(slot.color[0], slot.color[1], slot.color[2], slot.color[3]);
    let edge = if selected {
        Stroke::new(2.0, Color32::from_rgb(250, 220, 90))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 170))
    };
    for cell in slot.cells() {
        let scr = Rect::from_min_size(
            egui::pos2(origin.x + cell.x as f32 * scale, origin.y + cell.y as f32 * scale),
            egui::vec2(cell.w as f32 * scale, cell.h as f32 * scale),
        );
        painter.rect_filled(scr, 0.0, fill);
        painter.rect_stroke(scr, 0.0, edge);
    }
}

fn draw_checker(painter: &egui::Painter, rect: Rect) {
    let cell = 8.0;
    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    for j in 0..rows {
        for i in 0..cols {
            let c = if (i + j) % 2 == 0 { Color32::from_gray(58) } else { Color32::from_gray(44) };
            let r = Rect::from_min_size(
                egui::pos2(rect.min.x + i as f32 * cell, rect.min.y + j as f32 * cell),
                egui::vec2(cell, cell),
            );
            painter.rect_filled(r.intersect(rect), 0.0, c);
        }
    }
}

// ---- geometry -------------------------------------------------------------

pub fn slot_bounds(s: &Slot) -> RectF {
    let w = (s.grid.cols.max(1) - 1) as i32 * s.grid.pitch_x + s.rect.w;
    let h = (s.grid.rows.max(1) - 1) as i32 * s.grid.pitch_y + s.rect.h;
    RectF::new(s.rect.x, s.rect.y, w, h)
}

/// The layer's four corners in screen space, after rotation (TL, TR, BR, BL).
fn layer_corners_screen(layer: &Layer, origin: Pos2, scale: f32) -> [Pos2; 4] {
    let r = layer.rect;
    let (w, h) = (r.w as f32, r.h as f32);
    let (cx, cy) = (r.x as f32 + w * 0.5, r.y as f32 + h * 0.5);
    let center = egui::pos2(origin.x + cx * scale, origin.y + cy * scale);
    let ang = (layer.rotation as f32).to_radians();
    let (s, c) = (ang.sin(), ang.cos());
    let corner = |lx: f32, ly: f32| {
        let (dx, dy) = ((lx - cx) * scale, (ly - cy) * scale);
        egui::pos2(center.x + c * dx - s * dy, center.y + s * dx + c * dy)
    };
    [
        corner(r.x as f32, r.y as f32),
        corner((r.x + r.w) as f32, r.y as f32),
        corner((r.x + r.w) as f32, (r.y + r.h) as f32),
        corner(r.x as f32, (r.y + r.h) as f32),
    ]
}

fn aabb_of(pts: &[Pos2; 4]) -> Rect {
    let mut min = pts[0];
    let mut max = pts[0];
    for p in &pts[1..] {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Rect::from_min_max(min, max)
}

/// Is the screen-space point inside the layer's (possibly rotated) rectangle?
fn point_in_layer(layer: &Layer, p: Pos2, origin: Pos2, scale: f32) -> bool {
    let r = layer.rect;
    let (w, h) = (r.w as f32, r.h as f32);
    let (cx, cy) = (r.x as f32 + w * 0.5, r.y as f32 + h * 0.5);
    let center = egui::pos2(origin.x + cx * scale, origin.y + cy * scale);
    let (ox, oy) = (p.x - center.x, p.y - center.y);
    let ang = (layer.rotation as f32).to_radians();
    let (s, c) = (ang.sin(), ang.cos());
    // Inverse-rotate the offset back into the layer's local axes.
    let lx = (c * ox + s * oy) / scale;
    let ly = (-s * ox + c * oy) / scale;
    lx.abs() <= w * 0.5 && ly.abs() <= h * 0.5
}

/// For a Layer/Slot selection: (selection, its own rect, its bounding rect).
fn selection_target(app: &App) -> Option<(Selection, RectF, RectF)> {
    match app.selection {
        Some(Selection::Layer(id)) => app.project.layer(id).map(|l| (Selection::Layer(id), l.rect, l.rect)),
        Some(Selection::Slot(_)) => app.selected_slot_idx().map(|i| {
            let s = &app.project.slots[i];
            (Selection::Slot(s.id), s.rect, slot_bounds(s))
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_rounds_to_nearest_multiple() {
        assert_eq!(snap_to(13, 8), 16);
        assert_eq!(snap_to(10, 8), 8);
        assert_eq!(snap_to(-10, 8), -8);
        assert_eq!(snap_to(7, 1), 7);
    }

    #[test]
    fn move_rounds_to_whole_pixels() {
        let start = RectF::new(10, 20, 16, 16);
        assert_eq!(resolve_move(start, egui::vec2(5.4, -3.6), None), (15, 16));
    }

    #[test]
    fn move_snaps_when_enabled() {
        let start = RectF::new(10, 20, 16, 16);
        assert_eq!(resolve_move(start, egui::vec2(5.4, -3.6), Some(8)), (16, 16));
    }

    #[test]
    fn resize_clamps_and_snaps() {
        let start = RectF::new(0, 0, 16, 16);
        assert_eq!(resolve_resize(start, egui::vec2(10.0, -100.0), None), (26, 1));
        assert_eq!(resolve_resize(start, egui::vec2(10.0, 0.0), Some(8)), (24, 16));
    }

    #[test]
    fn axis_lock_freezes_off_axis_at_anchor() {
        let raw = egui::vec2(10.0, 3.0);
        let anchor = egui::vec2(2.0, 1.0);
        assert_eq!(lock_axis(raw, anchor, Axis::Horizontal), egui::vec2(10.0, 1.0));
        assert_eq!(lock_axis(raw, anchor, Axis::Vertical), egui::vec2(2.0, 3.0));
    }

    fn rotated_tl(r: RectF, deg: i32) -> (f32, f32) {
        let a = (deg as f32).to_radians();
        let (s, c) = (a.sin(), a.cos());
        let cx = r.x as f32 + r.w as f32 * 0.5;
        let cy = r.y as f32 + r.h as f32 * 0.5;
        let (dx, dy) = (-(r.w as f32) * 0.5, -(r.h as f32) * 0.5);
        (cx + c * dx - s * dy, cy + s * dx + c * dy)
    }

    #[test]
    fn resize_unrotated_keeps_topleft() {
        assert_eq!(resize_anchor_topleft(RectF::new(10, 20, 16, 16), 0, 30, 40), (10, 20));
    }

    #[test]
    fn resize_keeps_rotated_topleft_fixed() {
        let start = RectF::new(10, 20, 16, 16);
        for deg in [0, 30, 90, 200, 359] {
            let (x, y) = resize_anchor_topleft(start, deg, 40, 8);
            let (ax, ay) = rotated_tl(start, deg);
            let (bx, by) = rotated_tl(RectF::new(x, y, 40, 8), deg);
            // Top-left corner must stay put (within integer-rounding tolerance).
            assert!((ax - bx).abs() <= 1.0 && (ay - by).abs() <= 1.0, "deg {deg}: ({ax},{ay}) vs ({bx},{by})");
        }
    }
}
