//! UI / HUD geometry: builds the hotbar, open-inventory panel, per-slot item
//! icons, stack-count digits and the drag cursor each frame from the renderer's
//! [`UiSnapshot`] into a reusable dynamic vertex buffer of [`UiVertex`]s.
//!
//! ALL layout math (GUI scale, slot rectangles, panel placement) lives in this
//! one module so the renderer and the [`slot_at_cursor`] hit-test used by the App
//! (contract §9) can never diverge. Coordinates are computed in **physical
//! pixels** (origin top-left, y down) and converted to NDC only when emitting
//! vertices, so the pixel-space slot rects are the shared source of truth.
//!
//! Two kinds of geometry come out of [`build_ui`]:
//! - **gui quads** ([`UiBuild::verts`]): textured GUI sprites + solid-color fills
//!   plus font digits, drawn by `ui_pipe` with the gui atlas. Emitted
//!   back-to-front (dim background, panel/hotbar, selection, then digits) so later
//!   quads paint over earlier ones.
//! - **icon draws** ([`UiBuild::icons`]): per-slot item icons drawn by the
//!   `model3d_pipe` (isometric cube for `BlockCube`, flat tile quad for `Sprite`)
//!   with a per-icon MVP written into the model3d dynamic-offset uniform. These
//!   paint between the gui background and the digits.

use glam::{Mat4, Vec3};

use super::block_model::{push_billboard_quad, push_cube_textured};
use super::renderer::UiSnapshot;
use super::resources::GuiSprite;
use super::ui_text;
use crate::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
use crate::item::{ItemRenderKind, ItemType};
use crate::mesh::Vertex;

/// A single UI vertex: NDC position (y up) + gui-atlas uv + RGBA color. `uv.x < 0`
/// is the solid-color sentinel (see `ui.wgsl`). 32 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// uv sentinel marking a solid-color quad (no atlas sample).
const SOLID_UV: [f32; 2] = [-1.0, -1.0];

// --- Shared layout constants (physical pixels at GUI scale 1). ---

/// Slot interior side (px) — the textured icon area.
const SLOT_PX: f32 = 16.0;
/// Closed-hotbar slot pitch (px): the `hotbar.png` widget is 182px = 20×9 + 2, so
/// its 9 slots sit at a 20px pitch (16px interior + 2px border each side).
const HOTBAR_PITCH: f32 = 20.0;
/// Open-inventory slot pitch (px): the classic `inventory.png` panel (176×166)
/// lays its slots at an 18px pitch (16px interior + 1px border each side). Used
/// for BOTH the open main grid AND the open hotbar row — NOT the closed widget.
const PANEL_PITCH: f32 = 18.0;
/// Left inset (px) from the hotbar sprite's left edge to the first slot interior.
const HOTBAR_SLOT_INSET: f32 = 3.0;
/// Bottom margin (px) from the screen bottom edge to the hotbar sprite.
const HOTBAR_BOTTOM_MARGIN: f32 = 1.0;

/// Classic inventory panel interior layout (px), relative to the 176×166 panel's
/// top-left. The main 3×9 grid starts at (8, 84) and the hotbar row at (8, 142),
/// matching the vanilla `inventory.png` art.
const PANEL_W: f32 = 176.0;
const PANEL_H: f32 = 166.0;
const PANEL_GRID_X: f32 = 8.0;
const PANEL_GRID_Y: f32 = 84.0;
const PANEL_HOTBAR_Y: f32 = 142.0;

/// One slot's pixel rectangle (interior, where the icon + digits go) and the GUI
/// scale that produced it. All in physical pixels, top-left origin, y down.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlotRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A per-slot item icon to draw with the model3d pipeline. The geometry lives in
/// the shared [`UiBuild::icon_verts`]/[`UiBuild::icon_indices`] buffers (reused
/// across frames); this records the sub-ranges plus the MVP placing it into the
/// slot's NDC rect. Ranges (not owned `Vec`s) so building the UI allocates nothing
/// per icon per frame.
#[derive(Copy, Clone, Debug)]
pub struct IconDraw {
    /// Index of the first vertex of this icon in [`UiBuild::icon_verts`].
    pub vert_start: u32,
    /// Number of vertices this icon contributes.
    pub vert_count: u32,
    /// Index of the first element of this icon in [`UiBuild::icon_indices`].
    /// The stored index VALUES are GLOBAL within [`UiBuild::icon_verts`] (they
    /// point at this icon's vertices via their absolute position in the shared
    /// vertex buffer), so the renderer draws every icon with a SINGLE shared
    /// `base_vertex` (the offset of the whole icon block past the hand) — never a
    /// per-icon base, which is what would otherwise drift consecutive icons.
    pub index_start: u32,
    /// Number of indices this icon contributes.
    pub index_count: u32,
    /// Clip-space transform mapping the unit-cube / billboard model into the slot.
    pub mvp: Mat4,
}

/// The CPU-built UI for this frame.
pub struct UiBuild {
    /// gui-atlas quads (sprites + solid fills + digits), in paint order.
    pub verts: Vec<UiVertex>,
    /// Per-slot item icons drawn via the model3d pipeline (paint over the gui
    /// background, under the digits). Also includes the drag-cursor icon last.
    /// Each entry references a sub-range of [`Self::icon_verts`]/[`Self::icon_indices`].
    pub icons: Vec<IconDraw>,
    /// Shared, reused model3d vertices for ALL icons this frame (cleared + refilled,
    /// capacity retained) — no per-icon `Vec` allocation.
    pub icon_verts: Vec<Vertex>,
    /// Shared, reused model3d indices for all icons this frame. Indices are LOCAL
    /// to each icon (0-based within its own vertices); the renderer applies the
    /// per-icon `base_vertex` when drawing.
    pub icon_indices: Vec<u32>,
    /// gui-atlas quads drawn AFTER the icons (stack-count digits + drag count),
    /// so digits read on top of the icons.
    pub overlay_verts: Vec<UiVertex>,
}

/// Integer GUI scale chosen from the screen height (vanilla-style auto scale):
/// one scale step per ~240 logical px of height, clamped to `1..=4`. Returned as
/// `f32` since all layout multiplies by it.
pub fn gui_scale(screen: (u32, u32)) -> f32 {
    let (w, h) = screen;
    let by_h = (h / 240).max(1);
    let by_w = (w / 320).max(1);
    by_h.min(by_w).clamp(1, 4) as f32
}

/// The hotbar sprite's top-left pixel position for `screen` at `scale`.
fn hotbar_origin(screen: (u32, u32), scale: f32) -> (f32, f32) {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    let (sw, sh) = GuiSprite::Hotbar.size_px();
    let bw = sw as f32 * scale;
    let bh = sh as f32 * scale;
    let x = (w - bw) * 0.5;
    let y = h - bh - HOTBAR_BOTTOM_MARGIN * scale;
    (x, y)
}

/// The hotbar widget's TOP edge in NDC (y up) for `screen` at the auto GUI scale.
/// Derived from the SAME [`hotbar_origin`] layout the hotbar sprite is drawn with,
/// so it is the single source of truth for "where the hotbar's top is" — used by
/// the first-person hand's clearance regression test to assert the held item/arm
/// never dips into the hotbar band. Returns `+1.0` (top of screen) for a degenerate
/// zero-size screen. Test-only (the renderer draws the hotbar from `hotbar_origin`
/// directly); exists so the hand test shares this module's layout as the threshold.
#[cfg(test)]
pub fn hotbar_top_ndc(screen: (u32, u32)) -> f32 {
    if screen.0 == 0 || screen.1 == 0 {
        return 1.0;
    }
    let scale = gui_scale(screen);
    let (_ox, oy) = hotbar_origin(screen, scale);
    // `oy` is the hotbar sprite's TOP in physical pixels (top-left origin, y down).
    1.0 - oy / screen.1 as f32 * 2.0
}

/// The inventory panel's top-left pixel position (centered) for `screen` at
/// `scale`. The 176×166 art sits in the top-left of the 256×256 sheet.
fn panel_origin(screen: (u32, u32), scale: f32) -> (f32, f32) {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    let pw = PANEL_W * scale;
    let ph = PANEL_H * scale;
    ((w - pw) * 0.5, (h - ph) * 0.5)
}

/// The interior pixel rect of slot `i` (`0..TOTAL_SLOTS`) for the current screen /
/// open state. When closed, only the 9 hotbar slots have a rect; main-grid slots
/// return `None`. The single source of truth for both rendering and hit-testing.
pub fn slot_rect(i: usize, screen: (u32, u32), open: bool, scale: f32) -> Option<SlotRect> {
    if i >= TOTAL_SLOTS {
        return None;
    }
    let s = scale;
    let interior = SLOT_PX * s;
    if !open {
        // Closed: only the hotbar row is interactive / drawn.
        if i >= HOTBAR_LEN {
            return None;
        }
        let (ox, oy) = hotbar_origin(screen, s);
        let x = ox + (HOTBAR_SLOT_INSET + i as f32 * HOTBAR_PITCH) * s;
        // The 22px-tall hotbar sprite has a 3px top border, so the 16px slot
        // interior starts HOTBAR_SLOT_INSET px below the bar top — centring the
        // icon vertically in the slot row (interior 3..19, bottom border 19..22).
        let y = oy + HOTBAR_SLOT_INSET * s;
        return Some(SlotRect {
            x,
            y,
            w: interior,
            h: interior,
        });
    }
    // Open: slots laid out within the inventory panel.
    let (ox, oy) = panel_origin(screen, s);
    if i < HOTBAR_LEN {
        // Hotbar row at the bottom of the panel (18px panel pitch, not 20px).
        let col = i as f32;
        let x = ox + (PANEL_GRID_X + col * PANEL_PITCH) * s;
        let y = oy + PANEL_HOTBAR_Y * s;
        Some(SlotRect {
            x,
            y,
            w: interior,
            h: interior,
        })
    } else {
        // Main 3×9 grid, slots 9..36 (18px panel pitch, not 20px).
        let g = i - HOTBAR_LEN;
        let col = (g % 9) as f32;
        let row = (g / 9) as f32;
        let x = ox + (PANEL_GRID_X + col * PANEL_PITCH) * s;
        let y = oy + (PANEL_GRID_Y + row * PANEL_PITCH) * s;
        Some(SlotRect {
            x,
            y,
            w: interior,
            h: interior,
        })
    }
}

/// The inventory slot index (`0..TOTAL_SLOTS`) under the cursor, or `None`.
///
/// Pure layout function shared with the App (contract §9): when the inventory is
/// closed only the 9 hotbar slots are hit-testable (main-grid clicks return
/// `None`); when open all 36 slots are. Uses the SAME [`slot_rect`] math the
/// renderer draws with, so a visible slot is always the slot that gets clicked.
pub fn slot_at_cursor(screen: (u32, u32), open: bool, cursor_px: (f32, f32)) -> Option<usize> {
    let scale = gui_scale(screen);
    let (px, py) = cursor_px;
    let limit = if open { TOTAL_SLOTS } else { HOTBAR_LEN };
    for i in 0..limit {
        if let Some(r) = slot_rect(i, screen, open, scale) {
            if px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h {
                return Some(i);
            }
        }
    }
    None
}

/// Whether `cursor_px` lies within the open inventory panel rectangle (the
/// classic 176×166 art, centred). Used by the App to tell a click on the panel
/// from a "confidently outside the inventory" click that throws the held stack.
/// Uses the same [`panel_origin`] placement and auto [`gui_scale`] the panel is
/// drawn with. Always `false` for a degenerate zero-size screen.
pub fn cursor_in_panel(screen: (u32, u32), cursor_px: (f32, f32)) -> bool {
    if screen.0 == 0 || screen.1 == 0 {
        return false;
    }
    let scale = gui_scale(screen);
    let (ox, oy) = panel_origin(screen, scale);
    let (px, py) = cursor_px;
    px >= ox && px < ox + PANEL_W * scale && py >= oy && py < oy + PANEL_H * scale
}

/// Convert a physical-pixel point (top-left origin, y down) to NDC (y up).
#[inline]
fn to_ndc(screen: (u32, u32), x: f32, y: f32) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0]
}

/// Push a textured gui-sprite quad covering pixel rect `(x,y,w,h)` with `color`.
fn push_sprite(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    sprite: GuiSprite,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
) {
    let [u0, v0, u1, v1] = sprite.rect();
    push_quad_uv(out, screen, x, y, w, h, [u0, v0], [u1, v1], color);
}

/// Push a solid-color quad covering pixel rect `(x,y,w,h)`.
fn push_solid(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
) {
    push_quad_uv(out, screen, x, y, w, h, SOLID_UV, SOLID_UV, color);
}

/// Push a quad covering pixel rect `(x,y,w,h)` with explicit uv corners (top-left
/// `uv_tl`, bottom-right `uv_br`). Two CCW triangles. y-down pixels → y-up NDC.
fn push_quad_uv(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv_tl: [f32; 2],
    uv_br: [f32; 2],
    color: [f32; 4],
) {
    let p_tl = to_ndc(screen, x, y);
    let p_tr = to_ndc(screen, x + w, y);
    let p_br = to_ndc(screen, x + w, y + h);
    let p_bl = to_ndc(screen, x, y + h);
    let uv_tr = [uv_br[0], uv_tl[1]];
    let uv_bl = [uv_tl[0], uv_br[1]];
    let v = |pos: [f32; 2], uv: [f32; 2]| UiVertex { pos, uv, color };
    // tl, bl, br, tl, br, tr
    out.push(v(p_tl, uv_tl));
    out.push(v(p_bl, uv_bl));
    out.push(v(p_br, uv_br));
    out.push(v(p_tl, uv_tl));
    out.push(v(p_br, uv_br));
    out.push(v(p_tr, uv_tr));
}

/// Build the full UI for `ui` this frame. `verts`/`overlay_verts`/`icons` are the
/// caller-owned reusable buffers (cleared, capacity retained).
pub fn build_ui(ui: &UiSnapshot, build: &mut UiBuild) {
    build.verts.clear();
    build.overlay_verts.clear();
    build.icons.clear();
    build.icon_verts.clear();
    build.icon_indices.clear();

    let screen = ui.screen;
    if screen.0 == 0 || screen.1 == 0 {
        return;
    }
    let scale = gui_scale(screen);

    // --- Background sprites (drawn first, under everything). ---
    if ui.open {
        // Dim the whole screen (~0.6 alpha black) behind the panel.
        push_solid(
            &mut build.verts,
            screen,
            0.0,
            0.0,
            screen.0 as f32,
            screen.1 as f32,
            [0.0, 0.0, 0.0, 0.6],
        );
        // The inventory panel, centered.
        let (ox, oy) = panel_origin(screen, scale);
        push_sprite(
            &mut build.verts,
            screen,
            GuiSprite::InventoryPanel,
            ox,
            oy,
            PANEL_W * scale,
            PANEL_H * scale,
            [1.0, 1.0, 1.0, 1.0],
        );
    } else {
        // The hotbar strip, centered at the bottom.
        let (ox, oy) = hotbar_origin(screen, scale);
        let (sw, sh) = GuiSprite::Hotbar.size_px();
        push_sprite(
            &mut build.verts,
            screen,
            GuiSprite::Hotbar,
            ox,
            oy,
            sw as f32 * scale,
            sh as f32 * scale,
            [1.0, 1.0, 1.0, 1.0],
        );
        // Selection highlight over the active slot.
        let active = ui.active.min(HOTBAR_LEN as u8 - 1) as usize;
        if let Some(r) = slot_rect(active, screen, false, scale) {
            let (selw, selh) = GuiSprite::HotbarSelection.size_px();
            let sw = selw as f32 * scale;
            let sh = selh as f32 * scale;
            // Center the selection sprite on the slot interior.
            let sx = r.x + r.w * 0.5 - sw * 0.5;
            let sy = r.y + r.h * 0.5 - sh * 0.5;
            push_sprite(
                &mut build.verts,
                screen,
                GuiSprite::HotbarSelection,
                sx,
                sy,
                sw,
                sh,
                [1.0, 1.0, 1.0, 1.0],
            );
        }
    }

    // --- Per-slot item icons + stack-count digits. ---
    let slot_count = if ui.open { TOTAL_SLOTS } else { HOTBAR_LEN };
    for i in 0..slot_count {
        let Some((item, count)) = ui.slots[i] else {
            continue;
        };
        if item == ItemType::Air || count == 0 {
            continue;
        }
        let Some(r) = slot_rect(i, screen, ui.open, scale) else {
            continue;
        };
        push_slot_icon(build, screen, item, r);
        if count > 1 {
            push_count(&mut build.overlay_verts, screen, count as u32, r, scale);
        }
    }

    // --- Drag cursor: the cursor-held stack icon, drawn last (on top). ---
    if ui.open {
        if let Some((item, count)) = ui.cursor {
            if item != ItemType::Air && count > 0 {
                let s = SLOT_PX * scale;
                let (cx, cy) = ui.cursor_px;
                // Center the icon on the cursor.
                let r = SlotRect {
                    x: cx - s * 0.5,
                    y: cy - s * 0.5,
                    w: s,
                    h: s,
                };
                push_slot_icon(build, screen, item, r);
                if count > 1 {
                    push_count(&mut build.overlay_verts, screen, count as u32, r, scale);
                }
            }
        }
    }
}

/// Append the model3d icon for `item` placed into slot pixel rect `r`. Geometry is
/// appended into the shared, reused `build.icon_verts`/`icon_indices` buffers (no
/// per-icon allocation); the recorded [`IconDraw`] holds the sub-range + MVP.
fn push_slot_icon(build: &mut UiBuild, screen: (u32, u32), item: ItemType, r: SlotRect) {
    let vert_start = build.icon_verts.len() as u32;
    let index_start = build.icon_indices.len() as u32;
    let mvp = match item.render_kind() {
        ItemRenderKind::BlockCube(block) => {
            // Unit cube centered on the origin, drawn with an isometric ortho MVP.
            push_cube_textured(
                &mut build.icon_verts,
                &mut build.icon_indices,
                block.tiles(),
                Vec3::splat(-0.5),
                1.0,
            );
            iso_icon_mvp(screen, r)
        }
        ItemRenderKind::Sprite(tile) => {
            // Flat tile billboard filling the slot (front-facing toward -Z viewer).
            push_billboard_quad(
                &mut build.icon_verts,
                &mut build.icon_indices,
                tile,
                Vec3::ZERO,
                1.0,
            );
            flat_icon_mvp(screen, r)
        }
    };
    build.icons.push(IconDraw {
        vert_start,
        vert_count: build.icon_verts.len() as u32 - vert_start,
        index_start,
        index_count: build.icon_indices.len() as u32 - index_start,
        mvp,
    });
}

/// The slot's center in NDC (y up). Shared by the iso + flat icon MVPs so an icon
/// is always anchored at the geometric centre of its slot rect.
fn slot_ndc_center(screen: (u32, u32), r: SlotRect) -> [f32; 2] {
    to_ndc(screen, r.x + r.w * 0.5, r.y + r.h * 0.5)
}

/// The slot's ANISOTROPIC clip-space half-extents `[hx, hy]` in NDC. NDC spans 2
/// units across BOTH the framebuffer width and height, but the framebuffer is not
/// square (e.g. 16:9), so a pixel size `p` maps to a DIFFERENT NDC extent on each
/// axis: `p/w*2` horizontally vs `p/h*2` vertically. To draw an on-screen SQUARE
/// of `p` pixels the clip-space scale MUST therefore differ per axis — the
/// half-extents are `[p/w, p/h]` (a uniform single factor would render wider than
/// tall on a 16:9 screen, squishing the icon). The on-screen pixel extent is
/// `hx*w == hy*h == p` on both axes, so the icon is square at any aspect ratio.
/// Because every slot shares the same interior pixel size, this returns the SAME
/// pair for every slot — per-slot MVPs still differ only by the centre translation.
fn ndc_half_extents(screen: (u32, u32), r: SlotRect) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [r.w / w, r.h / h]
}

/// Isometric orthographic MVP mapping a unit cube (centered on origin, spanning
/// ±0.5) into the slot's NDC rect, back-face culled so the iso view reads with NO
/// depth buffer (front faces overdraw back faces in submission order).
fn iso_icon_mvp(screen: (u32, u32), r: SlotRect) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    // Classic MC item iso: rotate +30° about X then 45° about Y. With the model3d
    // pipeline's CCW-front / back-face cull, this makes the cube's TOP face plus
    // two sides (NegX + PosZ) face the +Z viewer and the BOTTOM cull away — the
    // camera looks DOWN at the cube. (A -30° X tilt would invert this and show the
    // BOTTOM + sides, which is the bug we are fixing.)
    let rot = Mat4::from_rotation_x(30f32.to_radians()) * Mat4::from_rotation_y(45f32.to_radians());
    // A unit cube rotated this way spans ~sqrt(2) ≈ 1.414 across; scale so it fills
    // ~0.9 of the slot. The clip-space scale is ANISOTROPIC (`sx != sy` on a
    // non-square framebuffer): each axis uses its own NDC half-extent so the cube
    // renders as an on-screen SQUARE of slot pixels at any aspect ratio. A single
    // uniform factor here would stretch the cube wider than tall on a 16:9 screen.
    let model_half = std::f32::consts::SQRT_2 * 0.5; // ~0.707
    let fill = 0.9;
    let [hx, hy] = ndc_half_extents(screen, r);
    let sx = hx * fill / model_half;
    let sy = hy * fill / model_half;
    // Map: clip = center + rotated_pos * scale, with y up. The cube has no depth
    // attachment, but the rasterizer still clips on clip-z in [0, 1] (wgpu), so
    // translate z to 0.5 and compress the rotated cube's z-extent into a tiny band
    // so it always stays inside [0, 1] regardless of slot size.
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(sx, sy, sx * 0.05))
        * rot
}

/// Orthographic MVP mapping the flat (X/Y plane) billboard quad (spanning ±0.5)
/// into the slot's NDC rect, facing the viewer.
fn flat_icon_mvp(screen: (u32, u32), r: SlotRect) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    // Flat sprite fills the slot. The billboard spans ±0.5 in model space, so a
    // per-axis scale of `2 × half_extent` maps it onto the full slot square. The
    // scale is ANISOTROPIC (`sx != sy` on a non-square framebuffer) so the sprite
    // renders as an on-screen SQUARE of slot pixels at any aspect ratio — a single
    // uniform factor would squish it wider than tall on a 16:9 screen.
    let [hx, hy] = ndc_half_extents(screen, r);
    let sx = hx * 2.0;
    let sy = hy * 2.0;
    // z translated to 0.5 so the flat quad sits inside the [0, 1] clip-z band.
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(sx, sy, 0.05))
}

/// Append the stack-count digits for `count` at the bottom-right of slot `r`.
/// Drawn as small solid white quads (one per lit font cell) with a 1px-offset
/// dark drop shadow for legibility, using the tiny 3×5 bitmap font.
fn push_count(out: &mut Vec<UiVertex>, screen: (u32, u32), count: u32, r: SlotRect, scale: f32) {
    // Font "pixel" size: scale up so digits read at the chosen GUI scale.
    let fp = scale.max(1.0);
    let num_w = ui_text::number_width(count) as f32 * fp;
    let num_h = ui_text::GLYPH_H as f32 * fp;
    // Bottom-right corner of the slot, nudged in by ~1 font-pixel.
    let x0 = r.x + r.w - num_w - fp * 0.0;
    let y0 = r.y + r.h - num_h - fp * 0.0;
    let shadow = [0.0, 0.0, 0.0, 1.0];
    let white = [1.0, 1.0, 1.0, 1.0];
    // Drop shadow first (offset by 1 font-pixel down-right), then the glyphs.
    ui_text::for_each_lit_cell(count, |px, py| {
        let cx = x0 + px as f32 * fp;
        let cy = y0 + py as f32 * fp;
        push_solid(out, screen, cx + fp, cy + fp, fp, fp, shadow);
    });
    ui_text::for_each_lit_cell(count, |px, py| {
        let cx = x0 + px as f32 * fp;
        let cy = y0 + py as f32 * fp;
        push_solid(out, screen, cx, cy, fp, fp, white);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_open(open: bool) -> UiSnapshot {
        let mut s = UiSnapshot {
            open,
            screen: (1280, 720),
            cursor_px: (640.0, 360.0),
            active: 2,
            slots: [None; TOTAL_SLOTS],
            cursor: None,
        };
        // A block-cube item and a sprite item in the hotbar.
        s.slots[0] = Some((ItemType::Stone, 64));
        s.slots[7] = Some((ItemType::Poppy, 1));
        s
    }

    #[test]
    fn gui_scale_is_clamped_and_increases_with_height() {
        assert_eq!(gui_scale((320, 240)), 1.0);
        assert!(gui_scale((1920, 1080)) >= 2.0);
        // Tiny screens never go below 1.
        assert_eq!(gui_scale((10, 10)), 1.0);
        // Huge screens cap at 4.
        assert_eq!(gui_scale((10000, 10000)), 4.0);
    }

    #[test]
    fn closed_hotbar_slots_have_rects_main_grid_does_not() {
        let scale = gui_scale((1280, 720));
        for i in 0..HOTBAR_LEN {
            assert!(
                slot_rect(i, (1280, 720), false, scale).is_some(),
                "hotbar {i}"
            );
        }
        for i in HOTBAR_LEN..TOTAL_SLOTS {
            assert!(
                slot_rect(i, (1280, 720), false, scale).is_none(),
                "main {i}"
            );
        }
    }

    #[test]
    fn open_has_all_36_slot_rects() {
        let scale = gui_scale((1280, 720));
        for i in 0..TOTAL_SLOTS {
            assert!(slot_rect(i, (1280, 720), true, scale).is_some(), "slot {i}");
        }
    }

    /// BUG 2: the OPEN inventory uses the classic 176×166 panel's 18px slot pitch
    /// (NOT the closed hotbar widget's 20px), so all 9 columns stay inside the 176px
    /// panel. With the wrong 20px pitch the 9th slot fell off the panel's right edge.
    #[test]
    fn open_slots_use_panel_pitch_and_fit_within_panel() {
        let screen = (1280u32, 720u32);
        let scale = gui_scale(screen);
        let (ox, _oy) = panel_origin(screen, scale);
        let panel_right = ox + PANEL_W * scale;
        // Open hotbar row AND the main grid: the 9th column's right edge fits inside.
        for &base in &[0usize, HOTBAR_LEN] {
            let last = base + 8; // 9th slot in the row.
            let r = slot_rect(last, screen, true, scale).unwrap();
            assert!(
                r.x + r.w <= panel_right + 1e-3,
                "open 9th slot (base {base}) right {} exceeds panel right {panel_right}",
                r.x + r.w
            );
            // Adjacent open slots step by exactly the 18px panel pitch (× scale).
            let r0 = slot_rect(base, screen, true, scale).unwrap();
            let r1 = slot_rect(base + 1, screen, true, scale).unwrap();
            assert!(
                (r1.x - r0.x - PANEL_PITCH * scale).abs() < 1e-3,
                "open pitch must be {PANEL_PITCH}px"
            );
        }
        // Classic coords: first main-grid slot at panel (8,84), hotbar row at (8,142).
        let (gx, gy) = (
            ox + PANEL_GRID_X * scale,
            panel_origin(screen, scale).1 + PANEL_GRID_Y * scale,
        );
        let grid0 = slot_rect(HOTBAR_LEN, screen, true, scale).unwrap();
        assert!(
            (grid0.x - gx).abs() < 1e-3 && (grid0.y - gy).abs() < 1e-3,
            "main grid origin (8,84)"
        );
        let hb0 = slot_rect(0, screen, true, scale).unwrap();
        let hy = panel_origin(screen, scale).1 + PANEL_HOTBAR_Y * scale;
        assert!(
            (hb0.x - gx).abs() < 1e-3 && (hb0.y - hy).abs() < 1e-3,
            "hotbar row origin (8,142)"
        );
        // Slot interior is the classic 16px (× scale).
        assert!(
            (grid0.w - SLOT_PX * scale).abs() < 1e-3,
            "slot interior 16px"
        );
    }

    /// BUG 2: the CLOSED hotbar widget keeps the 20px pitch of the 182px `hotbar.png`
    /// (= 20×9 + 2), so its 9 slots stay inside the widget. Confirms the two pitches
    /// are genuinely different and the closed one is the wider 20px.
    #[test]
    fn closed_hotbar_uses_widget_pitch() {
        let screen = (1280u32, 720u32);
        let scale = gui_scale(screen);
        let r0 = slot_rect(0, screen, false, scale).unwrap();
        let r1 = slot_rect(1, screen, false, scale).unwrap();
        assert!(
            (r1.x - r0.x - HOTBAR_PITCH * scale).abs() < 1e-3,
            "closed hotbar pitch must be {HOTBAR_PITCH}px"
        );
        // The 9th slot stays within the 182px hotbar widget.
        let (ox, _oy) = hotbar_origin(screen, scale);
        let (sw, _sh) = GuiSprite::Hotbar.size_px();
        let widget_right = ox + sw as f32 * scale;
        let r8 = slot_rect(8, screen, false, scale).unwrap();
        assert!(
            r8.x + r8.w <= widget_right + 1e-3,
            "closed 9th slot fits in widget"
        );
    }

    #[test]
    fn slot_at_cursor_round_trips_slot_centers() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        // Closed: hotbar slots hit, main grid never.
        for i in 0..HOTBAR_LEN {
            let r = slot_rect(i, screen, false, scale).unwrap();
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert_eq!(slot_at_cursor(screen, false, c), Some(i));
        }
        // Open: all 36 hit.
        for i in 0..TOTAL_SLOTS {
            let r = slot_rect(i, screen, true, scale).unwrap();
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert_eq!(slot_at_cursor(screen, true, c), Some(i));
        }
    }

    #[test]
    fn slot_at_cursor_misses_outside_any_slot() {
        // Top-left corner is outside every slot in both states.
        assert_eq!(slot_at_cursor((1280, 720), false, (0.0, 0.0)), None);
        assert_eq!(slot_at_cursor((1280, 720), true, (0.0, 0.0)), None);
    }

    #[test]
    fn cursor_in_panel_matches_panel_rect() {
        let screen = (1280u32, 720u32);
        let scale = gui_scale(screen);
        let (ox, oy) = panel_origin(screen, scale);
        // Panel centre is inside.
        assert!(cursor_in_panel(
            screen,
            (ox + PANEL_W * scale * 0.5, oy + PANEL_H * scale * 0.5)
        ));
        // Screen corner is confidently outside.
        assert!(!cursor_in_panel(screen, (0.0, 0.0)));
        // Just past the right edge is outside.
        assert!(!cursor_in_panel(
            screen,
            (ox + PANEL_W * scale + 1.0, oy + 1.0)
        ));
        // Every open slot lies within the panel it's drawn in.
        for i in 0..TOTAL_SLOTS {
            let r = slot_rect(i, screen, true, scale).unwrap();
            assert!(
                cursor_in_panel(screen, (r.x + r.w * 0.5, r.y + r.h * 0.5)),
                "slot {i} centre should be inside the panel"
            );
        }
        // Degenerate screen: never inside.
        assert!(!cursor_in_panel((0, 0), (0.0, 0.0)));
    }

    /// An empty reusable build buffer for the tests.
    fn empty_build() -> UiBuild {
        UiBuild {
            verts: Vec::new(),
            icons: Vec::new(),
            icon_verts: Vec::new(),
            icon_indices: Vec::new(),
            overlay_verts: Vec::new(),
        }
    }

    #[test]
    fn closed_build_has_hotbar_sprite_and_icons() {
        let mut build = empty_build();
        build_ui(&snap_open(false), &mut build);
        // Hotbar sprite + selection = at least 2 quads (12 verts).
        assert!(build.verts.len() >= 12);
        // Two items -> two icons (a cube + a sprite billboard).
        assert_eq!(build.icons.len(), 2);
        // Icon geometry lives in the shared reused buffers, ranges cover it.
        let cube = &build.icons[0];
        let sprite = &build.icons[1];
        assert_eq!(cube.vert_count, 24, "block-cube icon = 24 verts");
        assert_eq!(cube.index_count, 36);
        assert_eq!(sprite.vert_count, 8, "sprite icon = double-sided billboard");
        assert_eq!(sprite.index_count, 12);
        // The ranges tile the shared buffers without overlap (cube then sprite).
        assert_eq!(cube.vert_start, 0);
        assert_eq!(sprite.vert_start, cube.vert_count);
        assert_eq!(
            build.icon_verts.len() as u32,
            cube.vert_count + sprite.vert_count
        );
        assert_eq!(
            build.icon_indices.len() as u32,
            cube.index_count + sprite.index_count
        );
        // Stone stack of 64 (>1) emits digit quads in the overlay.
        assert!(!build.overlay_verts.is_empty());
    }

    #[test]
    fn open_build_dims_and_draws_panel() {
        let mut build = empty_build();
        let mut s = snap_open(true);
        s.cursor = Some((ItemType::Dirt, 12));
        build_ui(&s, &mut build);
        // Dim quad + panel sprite = at least 12 verts.
        assert!(build.verts.len() >= 12);
        // Two slot items + the drag cursor icon = 3 icons.
        assert_eq!(build.icons.len(), 3);
    }

    #[test]
    fn build_ui_reuses_icon_buffers_without_growth() {
        // The shared icon buffers are cleared + refilled each frame, never
        // reallocated, mirroring the per-frame no-allocation performance rule.
        let mut build = empty_build();
        build_ui(&snap_open(true), &mut build);
        let vcap = build.icon_verts.capacity();
        let icap = build.icon_indices.capacity();
        assert!(vcap > 0, "first build should populate icon geometry");
        // Rebuild with the closed (smaller) UI: cleared + refilled, capacity kept.
        build_ui(&snap_open(false), &mut build);
        assert_eq!(build.icon_verts.capacity(), vcap, "icon vert buffer reused");
        assert_eq!(
            build.icon_indices.capacity(),
            icap,
            "icon index buffer reused"
        );
    }

    #[test]
    fn iso_mvp_keeps_cube_within_clip_xy() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        let r = slot_rect(0, screen, false, scale).unwrap();
        let mvp = iso_icon_mvp(screen, r);
        // The 8 cube corners must land inside [-1, 1] in NDC x/y and clip-z [0, 1].
        for &x in &[-0.5f32, 0.5] {
            for &y in &[-0.5f32, 0.5] {
                for &z in &[-0.5f32, 0.5] {
                    let c = mvp * glam::Vec4::new(x, y, z, 1.0);
                    assert!(c.x.abs() <= 1.0 + 1e-3, "x {} out of clip", c.x);
                    assert!(c.y.abs() <= 1.0 + 1e-3, "y {} out of clip", c.y);
                    assert!((0.0..=1.0).contains(&c.z), "z {} out of clip-z band", c.z);
                }
            }
        }
    }

    /// BUG 1: the iso view must show the cube's TOP face (camera looks DOWN at it),
    /// not the bottom. The fix flips the X tilt from -30° to +30°, which swaps which
    /// cube faces present toward the viewer. The model3d rasterizer decides facing
    /// by a normal's clip-z sign, so we assert (a) the SHIPPED iso orients the +Y
    /// (top) normal opposite to the -Y (bottom) normal, and (b) flipping the tilt
    /// sign swaps which of top/bottom carries the viewer-facing sign — pinning that
    /// the shipped +30° tilt presents the top where the old -30° presented the
    /// bottom. (Anchor-free: it only asserts the swap the fix makes.)
    #[test]
    fn iso_mvp_flips_to_show_top_face() {
        let screen = (1280u32, 720u32);
        let r = slot_rect(0, screen, false, gui_scale(screen)).unwrap();
        let shipped = iso_icon_mvp(screen, r); // +30° tilt (the fix)
        let z_of = |m: Mat4, n: Vec3| (m * n.extend(0.0)).z;
        // Within the shipped iso, top and bottom face opposite ways.
        assert!(
            z_of(shipped, Vec3::Y) * z_of(shipped, Vec3::NEG_Y) < 0.0,
            "top and bottom normals must point opposite ways in the iso view"
        );
        // The old (buggy) iso differed ONLY by the X-tilt sign. Rebuild it the same
        // way `iso_icon_mvp` composes, but with -30°, and confirm the tilt flip
        // swaps top/bottom: the shipped +30° top shares the old -30° bottom's sign.
        let center = slot_ndc_center(screen, r);
        let [hx, hy] = ndc_half_extents(screen, r);
        let model_half = std::f32::consts::SQRT_2 * 0.5;
        let sx = hx * 0.9 / model_half;
        let sy = hy * 0.9 / model_half;
        let rot_minus = Mat4::from_rotation_x(-(30f32.to_radians()))
            * Mat4::from_rotation_y(45f32.to_radians());
        let old = Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
            * Mat4::from_scale(Vec3::new(sx, sy, sx * 0.05))
            * rot_minus;
        assert_eq!(
            z_of(shipped, Vec3::Y).signum(),
            z_of(old, Vec3::NEG_Y).signum(),
            "the +30° fix must show the top where the old -30° showed the bottom"
        );
        assert_ne!(
            z_of(shipped, Vec3::Y).signum(),
            z_of(old, Vec3::Y).signum(),
            "flipping the tilt must flip the top face's viewer-facing sign"
        );
    }

    /// BUG 2: every slot's icon MVP must be IDENTICAL except for the per-slot
    /// centre translation — no accumulating drift, no per-slot scale/rotation
    /// difference. We strip the translation (set the last column to origin) from two
    /// different slots' MVPs and assert the remaining linear (rotation+scale) parts
    /// are equal, then assert the translations differ by exactly the slot-centre
    /// delta (so consecutive slots step by one slot pitch, not more).
    #[test]
    fn iso_mvp_differs_only_by_per_slot_translation() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        // Check across several slot pairs in both the closed hotbar and open grid.
        let check = |a: usize, b: usize, open: bool| {
            let ra = slot_rect(a, screen, open, scale).unwrap();
            let rb = slot_rect(b, screen, open, scale).unwrap();
            let ma = iso_icon_mvp(screen, ra);
            let mb = iso_icon_mvp(screen, rb);
            // Linear part (columns 0..3) must match exactly: identical scale + rot.
            for col in 0..3 {
                let ca = ma.col(col);
                let cb = mb.col(col);
                assert!(
                    (ca - cb).length() < 1e-6,
                    "linear column {col} differs between slots {a},{b} (open={open}): {ca:?} vs {cb:?}"
                );
            }
            // Translation column differs by exactly the slot-centre NDC delta.
            let ca = slot_ndc_center(screen, ra);
            let cb = slot_ndc_center(screen, rb);
            let dx = mb.col(3).x - ma.col(3).x;
            let dy = mb.col(3).y - ma.col(3).y;
            assert!(
                (dx - (cb[0] - ca[0])).abs() < 1e-6,
                "x translation drift slots {a},{b}"
            );
            assert!(
                (dy - (cb[1] - ca[1])).abs() < 1e-6,
                "y translation drift slots {a},{b}"
            );
        };
        // Adjacent hotbar slots + a far pair (closed): no progressive drift.
        check(0, 1, false);
        check(0, 8, false);
        // Open grid: a hotbar slot, an adjacent grid slot, and a far grid slot.
        check(0, 1, true);
        check(9, 10, true);
        check(0, 35, true);
    }

    /// BUG 1 (squish): the clip-space scale must be ANISOTROPIC so an icon renders as
    /// the SAME on-screen pixel shape at any framebuffer aspect ratio. NDC spans 2
    /// units across BOTH framebuffer axes, so to map a slot of P pixels to equal
    /// on-screen extents the half-extents MUST be `[P/w, P/h]` — a single uniform NDC
    /// factor (the wrong round-1 fix) would render wider than tall on a 16:9 screen.
    ///
    /// For the FLAT sprite the model is a planar ±0.5 quad, so "square on screen"
    /// is exact: its on-screen extents equal the slot's pixel size on both axes at
    /// any aspect. For the ISO cube the projected silhouette is naturally taller than
    /// wide (the +30° tilt foreshortens the horizontal diagonal), so squareness is
    /// asserted as ASPECT-INDEPENDENCE: the on-screen pixel footprint ratio is
    /// IDENTICAL across screen aspects (the round-1 uniform version stretched it by
    /// ~the framebuffer aspect ratio, ~2× between square and 2:1).
    #[test]
    fn icon_mvp_is_square_on_screen_at_any_aspect() {
        // The on-screen pixel x/y span of an MVP applied to a unit model (±0.5 cube
        // for iso, ±0.5 quad for flat), via the framebuffer dims (NDC 2 == full dim).
        let pixel_extents = |mvp: Mat4, screen: (u32, u32), three_d: bool| -> (f32, f32) {
            let (w, h) = (screen.0 as f32, screen.1 as f32);
            let zs: &[f32] = if three_d { &[-0.5, 0.5] } else { &[0.0] };
            let mut min = glam::Vec2::splat(f32::INFINITY);
            let mut max = glam::Vec2::splat(f32::NEG_INFINITY);
            for &x in &[-0.5f32, 0.5] {
                for &y in &[-0.5f32, 0.5] {
                    for &z in zs {
                        let c = mvp * glam::Vec4::new(x, y, z, 1.0);
                        min = min.min(glam::Vec2::new(c.x, c.y));
                        max = max.max(glam::Vec2::new(c.x, c.y));
                    }
                }
            }
            ((max.x - min.x) * w * 0.5, (max.y - min.y) * h * 0.5)
        };
        // A fixed pixel-size slot so only the framebuffer aspect changes between runs.
        let r = SlotRect {
            x: 0.0,
            y: 0.0,
            w: 48.0,
            h: 48.0,
        };
        // Reference iso footprint aspect on a square screen.
        let (rx, ry) = pixel_extents(iso_icon_mvp((600, 600), r), (600, 600), true);
        let iso_ref_aspect = rx / ry;
        for &screen in &[(600u32, 600u32), (1280, 720), (1920, 1080)] {
            // Iso cube: on-screen footprint aspect is the SAME at every screen aspect.
            let (px, py) = pixel_extents(iso_icon_mvp(screen, r), screen, true);
            let aspect = px / py;
            assert!(
                (aspect - iso_ref_aspect).abs() < 1e-3,
                "iso icon footprint aspect changed with screen {screen:?}: {aspect} vs ref {iso_ref_aspect}"
            );
            // Flat sprite: the ±0.5 quad maps to the slot's exact 48px square — equal
            // on-screen pixel width/height (a true square) at any aspect.
            let (fx, fy) = pixel_extents(flat_icon_mvp(screen, r), screen, false);
            assert!(
                (fx - fy).abs() < 1e-3,
                "flat icon not square on screen {screen:?}: {fx}px wide vs {fy}px tall"
            );
            assert!(
                (fx - r.w).abs() < 1e-3,
                "flat icon px width {fx} != slot {}",
                r.w
            );
            assert!(
                (fy - r.h).abs() < 1e-3,
                "flat icon px height {fy} != slot {}",
                r.h
            );
        }
    }

    /// `hotbar_top_ndc` must equal the hotbar sprite's TOP edge converted to NDC
    /// from the SAME layout the sprite is drawn with — it is the threshold the
    /// first-person hand clearance test depends on, so pin it to the real layout.
    #[test]
    fn hotbar_top_ndc_matches_sprite_top() {
        for &screen in &[(1280u32, 720u32), (1920, 1080), (2560, 1440), (3840, 2160)] {
            let scale = gui_scale(screen);
            let (_ox, oy) = hotbar_origin(screen, scale);
            let expected = 1.0 - oy / screen.1 as f32 * 2.0;
            assert!(
                (hotbar_top_ndc(screen) - expected).abs() < 1e-6,
                "screen {screen:?}"
            );
            // The top edge sits in the lower band of the screen (well below centre).
            assert!(
                hotbar_top_ndc(screen) < 0.0,
                "hotbar top should be below NDC centre"
            );
        }
        // Degenerate zero-size screen returns the top of the screen (no hotbar).
        assert_eq!(hotbar_top_ndc((0, 0)), 1.0);
    }

    #[test]
    fn empty_screen_builds_nothing() {
        let mut build = empty_build();
        let s = UiSnapshot {
            screen: (0, 0),
            ..snap_open(false)
        };
        build_ui(&s, &mut build);
        assert!(build.verts.is_empty() && build.icons.is_empty());
        assert!(build.icon_verts.is_empty() && build.icon_indices.is_empty());
    }
}
