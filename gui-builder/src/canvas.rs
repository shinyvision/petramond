//! The preview canvas: the real petramond-ui frame rasterized by the software
//! renderer (pixel-exactly what the game shows), with editor chrome overlaid
//! as egui shapes — selection outline + resize handles, hover outline, pixel
//! grid, role badges, drag-to-move/resize/reorder.

use crate::app::App;
use crate::doc_edit::{self, NodePath};
use crate::preview::{self, RectEntry};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use petramond_ui::{Dir, InstKey, PreviewState, RectI, Size};

const SEL: Color32 = Color32::from_rgb(90, 170, 255);
const HOVER: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 90);
const CARET: Color32 = Color32::from_rgb(255, 200, 60);

/// What the cached preview texture was rendered for.
#[derive(Clone, PartialEq)]
pub struct CanvasKey {
    doc_rev: u64,
    theme_rev: u64,
    screen: (u32, u32),
    scale: i32,
    forced: (Option<String>, bool, bool, bool),
}

pub enum CanvasDrag {
    Move { path: NodePath, orig: (i32, i32), start: Pos2 },
    Resize { path: NodePath, handle: Handle, orig: (i32, i32), start: Pos2 },
    Reorder { path: NodePath, insert: usize },
}

#[derive(Clone, Copy, PartialEq)]
pub enum Handle {
    E,
    S,
    Se,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let screen = app.proj.editor.screen;
    let scale = app.proj.editor.preview_scale.clamp(1, 4) as i32;
    let zoom = app.proj.editor.zoom.clamp(0.25, 16.0);
    let ppl = scale as f32 * zoom; // screen px per logical px

    // Ctrl+wheel zooms the canvas.
    if ui.rect_contains_pointer(ui.max_rect()) {
        let (ctrl, scroll) = ui.input(|i| (i.modifiers.ctrl, i.raw_scroll_delta.y));
        if ctrl && scroll != 0.0 {
            let z = app.proj.editor.zoom * if scroll > 0.0 { 1.25 } else { 0.8 };
            app.proj.editor.zoom = z.clamp(0.25, 16.0);
        }
    }

    ensure_texture(app, ui.ctx(), screen, scale);

    let state = app.preview_state();
    let viewport = ((screen.0 as i32) / scale, (screen.1 as i32) / scale);
    let rects = preview::layout_rects(
        &app.proj.document,
        app.theme.theme.as_ref(),
        &state,
        &app.images,
        viewport,
    );

    egui::ScrollArea::both()
        .id_salt("canvas_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let size = Vec2::new(screen.0 as f32 * zoom, screen.1 as f32 * zoom);
            let (rect, response) =
                ui.allocate_exact_size(size + Vec2::splat(16.0), Sense::click_and_drag());
            let origin = rect.min + Vec2::splat(8.0);
            let canvas = Rect::from_min_size(origin, size);
            let painter = ui.painter_at(ui.clip_rect());
            painter.rect_filled(canvas.expand(1.0), 0.0, Color32::from_gray(10));
            if let Some(tex) = &app.canvas_tex {
                painter.image(
                    tex.id(),
                    canvas,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }

            let to_screen = |r: RectI| {
                Rect::from_min_size(
                    origin + Vec2::new(r.x as f32 * ppl, r.y as f32 * ppl),
                    Vec2::new(r.w as f32 * ppl, r.h as f32 * ppl),
                )
            };

            if app.overlay {
                if app.proj.editor.pixel_grid && ppl >= 8.0 {
                    draw_pixel_grid(&painter, canvas, ui.clip_rect(), ppl);
                }
                for e in &rects {
                    if e.slot_role.is_some() {
                        let r = to_screen(e.rect);
                        painter.text(
                            r.left_top() + Vec2::new(2.0, -2.0),
                            egui::Align2::LEFT_BOTTOM,
                            e.slot_role.as_deref().unwrap_or(""),
                            egui::FontId::monospace(10.0),
                            Color32::from_rgba_premultiplied(255, 220, 120, 200),
                        );
                    }
                }
            }

            interact(app, ui, &response, &rects, origin, ppl);

            // Overlay chrome after interaction so it reflects this frame.
            if app.overlay {
                let ptr = response.hover_pos();
                if app.canvas_drag.is_none() {
                    if let Some(p) = ptr {
                        if let Some(e) = topmost_at(&rects, from_screen(p, origin, ppl)) {
                            painter.rect_stroke(to_screen(e.rect), 0.0, Stroke::new(1.0, HOVER));
                        }
                    }
                }
                if let Some(sel) = app.sel.clone() {
                    if let Some(e) = rects.iter().find(|e| e.path == sel) {
                        let r = to_screen(e.rect);
                        painter.rect_stroke(r, 0.0, Stroke::new(2.0, SEL));
                        for (h, pos) in handle_positions(r) {
                            let hr = Rect::from_center_size(pos, Vec2::splat(7.0));
                            painter.rect_filled(hr, 1.0, SEL);
                            let _ = h;
                        }
                    }
                }
                if let Some(CanvasDrag::Reorder { path, insert }) = &app.canvas_drag {
                    draw_insert_caret(app, &painter, &rects, path, *insert, &to_screen);
                }
            }
        });
}

fn from_screen(p: Pos2, origin: Pos2, ppl: f32) -> (i32, i32) {
    (
        ((p.x - origin.x) / ppl).floor() as i32,
        ((p.y - origin.y) / ppl).floor() as i32,
    )
}

/// The topmost (last expanded) node whose rect contains the logical point.
fn topmost_at<'a>(rects: &'a [RectEntry], p: (i32, i32)) -> Option<&'a RectEntry> {
    rects
        .iter()
        .rev()
        .find(|e| e.rect.w > 0 && e.rect.h > 0 && e.rect.contains(p.0, p.1))
}

fn handle_positions(r: Rect) -> [(Handle, Pos2); 3] {
    [
        (Handle::E, Pos2::new(r.max.x, r.center().y)),
        (Handle::S, Pos2::new(r.center().x, r.max.y)),
        (Handle::Se, r.max),
    ]
}

fn interact(
    app: &mut App,
    ui: &egui::Ui,
    response: &egui::Response,
    rects: &[RectEntry],
    origin: Pos2,
    ppl: f32,
) {
    let ptr = response.interact_pointer_pos().or_else(|| response.hover_pos());

    if response.clicked() {
        if let Some(p) = ptr {
            let hit = topmost_at(rects, from_screen(p, origin, ppl)).map(|e| e.path.clone());
            app.select_external(hit);
        }
    }
    if response.double_clicked() {
        if let Some(p) = ptr {
            if let Some(e) = topmost_at(rects, from_screen(p, origin, ppl)) {
                app.select_external(Some(e.path.clone()));
                if matches!(e.type_name, "label" | "button" | "badge" | "alert" | "text_input") {
                    app.focus_text_edit = true;
                }
            }
        }
    }

    if response.drag_started() {
        if let Some(p) = ptr {
            app.canvas_drag = classify_drag(app, rects, p, origin, ppl);
        }
    }

    if response.dragged() {
        let Some(p) = ptr else { return };
        match app.canvas_drag.take() {
            Some(CanvasDrag::Move { path, orig, start }) => {
                let dx = ((p.x - start.x) / ppl).round() as i32;
                let dy = ((p.y - start.y) / ppl).round() as i32;
                let want = (orig.0 + dx, orig.1 + dy);
                let cur = doc_edit::node_at(&app.proj.document.root, &path)
                    .and_then(|n| n.layout.abs)
                    .map(|a| (a.x, a.y));
                if cur != Some(want) {
                    let path2 = path.clone();
                    app.gesture_mutate(move |doc| {
                        if let Some(n) = doc_edit::node_at_mut(&mut doc.root, &path2) {
                            n.layout.abs = Some(petramond_ui::AbsPos { x: want.0, y: want.1 });
                        }
                    });
                }
                app.canvas_drag = Some(CanvasDrag::Move { path, orig, start });
            }
            Some(CanvasDrag::Resize { path, handle, orig, start }) => {
                let dx = ((p.x - start.x) / ppl).round() as i32;
                let dy = ((p.y - start.y) / ppl).round() as i32;
                let w = (orig.0 + dx).max(1);
                let h = (orig.1 + dy).max(1);
                let path2 = path.clone();
                app.gesture_mutate(move |doc| {
                    if let Some(n) = doc_edit::node_at_mut(&mut doc.root, &path2) {
                        if matches!(handle, Handle::E | Handle::Se) {
                            n.layout.w = Size::Px(w);
                        }
                        if matches!(handle, Handle::S | Handle::Se) {
                            n.layout.h = Size::Px(h);
                        }
                    }
                });
                app.canvas_drag = Some(CanvasDrag::Resize { path, handle, orig, start });
            }
            Some(CanvasDrag::Reorder { path, .. }) => {
                let insert = reorder_insert_index(app, rects, &path, p, origin, ppl);
                app.canvas_drag = Some(CanvasDrag::Reorder { path, insert });
            }
            None => {}
        }
        ui.ctx().request_repaint();
    }

    if response.drag_stopped() {
        if let Some(CanvasDrag::Reorder { path, insert }) = app.canvas_drag.take() {
            if !path.is_empty() {
                let parent = path[..path.len() - 1].to_vec();
                let cur = *path.last().unwrap();
                // Moving to its own position (or just after) is a no-op.
                if insert != cur && insert != cur + 1 {
                    let mut new_sel = None;
                    app.mutate(|doc| {
                        new_sel = doc_edit::move_node(&mut doc.root, &path, &parent, insert);
                    });
                    if new_sel.is_some() {
                        app.sel = new_sel;
                    }
                }
            }
        }
    }
}

fn classify_drag(
    app: &App,
    rects: &[RectEntry],
    p: Pos2,
    origin: Pos2,
    ppl: f32,
) -> Option<CanvasDrag> {
    // Resize handles of the current selection win over node picking.
    if let Some(sel) = &app.sel {
        if let Some(e) = rects.iter().find(|e| &e.path == sel) {
            let r = Rect::from_min_size(
                origin + Vec2::new(e.rect.x as f32 * ppl, e.rect.y as f32 * ppl),
                Vec2::new(e.rect.w as f32 * ppl, e.rect.h as f32 * ppl),
            );
            for (handle, pos) in handle_positions(r) {
                if pos.distance(p) <= 7.0 {
                    return Some(CanvasDrag::Resize {
                        path: sel.clone(),
                        handle,
                        orig: (e.rect.w, e.rect.h),
                        start: p,
                    });
                }
            }
        }
    }
    let e = topmost_at(rects, from_screen(p, origin, ppl))?;
    if e.path.is_empty() {
        return None; // the root doesn't move
    }
    if e.abs {
        let node = doc_edit::node_at(&app.proj.document.root, &e.path)?;
        let abs = node.layout.abs.unwrap_or_default();
        Some(CanvasDrag::Move { path: e.path.clone(), orig: (abs.x, abs.y), start: p })
    } else {
        Some(CanvasDrag::Reorder {
            path: e.path.clone(),
            insert: *e.path.last().unwrap(),
        })
    }
}

/// Flow siblings of `path` (first rect entry per sibling index, abs excluded).
fn flow_siblings(app: &App, rects: &[RectEntry], parent: &[usize]) -> Vec<(usize, RectI)> {
    let Some(parent_node) = doc_edit::node_at(&app.proj.document.root, parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..parent_node.children.len() {
        if parent_node.children[i].layout.abs.is_some() {
            continue;
        }
        let mut child = parent.to_vec();
        child.push(i);
        if let Some(e) = rects.iter().find(|e| e.path == child) {
            out.push((i, e.rect));
        }
    }
    out
}

fn parent_dir(app: &App, parent: &[usize]) -> Dir {
    doc_edit::node_at(&app.proj.document.root, parent)
        .map(|n| n.flow_dir())
        .unwrap_or(Dir::Column)
}

fn reorder_insert_index(
    app: &App,
    rects: &[RectEntry],
    path: &[usize],
    p: Pos2,
    origin: Pos2,
    ppl: f32,
) -> usize {
    let parent = &path[..path.len() - 1];
    let dir = parent_dir(app, parent);
    let (lx, ly) = (((p.x - origin.x) / ppl), ((p.y - origin.y) / ppl));
    let pointer_main = if dir == Dir::Row { lx } else { ly };
    let sibs = flow_siblings(app, rects, parent);
    let mut insert = 0;
    for (i, r) in &sibs {
        let center = if dir == Dir::Row {
            r.x as f32 + r.w as f32 / 2.0
        } else {
            r.y as f32 + r.h as f32 / 2.0
        };
        if pointer_main > center {
            insert = i + 1;
        }
    }
    insert
}

fn draw_insert_caret(
    app: &App,
    painter: &egui::Painter,
    rects: &[RectEntry],
    path: &[usize],
    insert: usize,
    to_screen: &dyn Fn(RectI) -> Rect,
) {
    let parent = &path[..path.len() - 1];
    let dir = parent_dir(app, parent);
    let sibs = flow_siblings(app, rects, parent);
    if sibs.is_empty() {
        return;
    }
    // The caret sits before the first sibling at/after `insert`, or after the
    // last one.
    let (rect, after) = match sibs.iter().find(|(i, _)| *i >= insert) {
        Some((_, r)) => (*r, false),
        None => (sibs.last().unwrap().1, true),
    };
    let r = to_screen(rect);
    let (a, b) = if dir == Dir::Row {
        let x = if after { r.max.x + 2.0 } else { r.min.x - 2.0 };
        (Pos2::new(x, r.min.y), Pos2::new(x, r.max.y))
    } else {
        let y = if after { r.max.y + 2.0 } else { r.min.y - 2.0 };
        (Pos2::new(r.min.x, y), Pos2::new(r.max.x, y))
    };
    painter.line_segment([a, b], Stroke::new(2.0, CARET));
}

fn draw_pixel_grid(painter: &egui::Painter, canvas: Rect, clip: Rect, ppl: f32) {
    let area = canvas.intersect(clip);
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return;
    }
    let stroke = Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 14));
    let mut x = canvas.min.x + ((area.min.x - canvas.min.x) / ppl).floor() * ppl;
    while x <= area.max.x {
        painter.line_segment([Pos2::new(x, area.min.y), Pos2::new(x, area.max.y)], stroke);
        x += ppl;
    }
    let mut y = canvas.min.y + ((area.min.y - canvas.min.y) / ppl).floor() * ppl;
    while y <= area.max.y {
        painter.line_segment([Pos2::new(area.min.x, y), Pos2::new(area.max.x, y)], stroke);
        y += ppl;
    }
}

fn ensure_texture(app: &mut App, ctx: &egui::Context, screen: (u32, u32), scale: i32) {
    let forced_id = app
        .selected_node()
        .and_then(|n| n.id.clone())
        .filter(|_| app.forced.hover || app.forced.pressed || app.forced.focus);
    let key = CanvasKey {
        doc_rev: app.doc_rev,
        theme_rev: app.theme.rev,
        screen,
        scale,
        forced: (forced_id.clone(), app.forced.hover, app.forced.pressed, app.forced.focus),
    };
    if app.canvas_tex.is_some() && app.canvas_tex_key.as_ref() == Some(&key) {
        return;
    }
    let forced = forced_id.map(|id| {
        let k = InstKey { id, item: None };
        PreviewState {
            hover: app.forced.hover.then(|| k.clone()),
            pressed: app.forced.pressed.then(|| k.clone()),
            focus: app.forced.focus.then(|| k.clone()),
        }
    });
    let state = app.preview_state();
    let rgba = preview::render_rgba(
        &app.proj.document,
        &app.theme.theme,
        &state,
        &app.images,
        screen,
        scale,
        forced.as_ref(),
    );
    let img = egui::ColorImage::from_rgba_unmultiplied(
        [screen.0 as usize, screen.1 as usize],
        &rgba,
    );
    match &mut app.canvas_tex {
        Some(tex) => tex.set(img, egui::TextureOptions::NEAREST),
        None => {
            app.canvas_tex = Some(ctx.load_texture("preview", img, egui::TextureOptions::NEAREST))
        }
    }
    app.canvas_tex_key = Some(key);
}
