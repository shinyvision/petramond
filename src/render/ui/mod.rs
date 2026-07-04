//! UI / HUD geometry: turns the renderer's [`UiSnapshot`] into the per-frame
//! vertex buffers for one GUI — the hotbar HUD, or an open inventory / crafting
//! table / furnace / chest panel.
//!
//! Every screen is DATA-DRIVEN: its panel art, slot positions, hover highlight and
//! dynamic overlays come from a baked manifest ([`crate::gui`]). There are no
//! per-screen layout modules — [`build_ui`] is generic over [`GuiKind`], reading
//! the open kind's [`GuiDef`] and the matching slice of game state from the
//! snapshot. The SAME def backs the App's click hit-test (`crate::gui::hit`), so
//! what's drawn and what's clicked can never diverge.
//!
//! All layout math is in **physical pixels** (origin top-left, y down) and
//! converted to NDC only when emitting vertices. [`build_ui`] fills, in paint
//! order back-to-front:
//! - `dim`: the fullscreen menu backdrop (solid; menus only).
//! - `panel`: the baked panel PNG quad (its own texture).
//! - `overlays`: dynamic string-tagged overlay quads clipped by game state
//!   (furnace gauges; mod overlays clipped by the GUI state map) plus mod
//!   widget art (`image`/`button`/`rotimage`), each its own texture —
//!   `overlay_spans` records the per-group `(sprite key, vertex count)` so the
//!   renderer binds the right texture per quad group.
//! - `hover`: the hover / selection highlight (its own texture), over the slot
//!   under the cursor — or, for the hotbar HUD, the active slot.
//! - `icon_quads`: one `(item, slot rect)` per filled slot (icon atlas).
//! - `counts`: stack-count digits (solid), over the icons.
//! - `drag_icon_quads` + `drag_counts`: the cursor-held stack, front-most.

// `pub(crate)` so the one-time icon-atlas bake (`renderer::icon_atlas`) can reach
// the `pub(crate)` MVP projection fns; the per-slot helpers stay `pub(super)`.
pub(crate) mod icon;

use crate::gui::{
    self as gui_layout, gui_scale, GuiKind, GuiValue, HoverFit, OverlayMode, Role, ShellKind,
    SlotRect, SpriteKey, WidgetDef,
};
use crate::gui::{
    ShellButton, ShellInput, ShellListRow, ShellScrollbar, ShellText, ShellTextAlign, UiSnapshot,
};
use crate::inventory::HOTBAR_LEN;
use crate::item::ItemType;

/// A single UI vertex: NDC position (y up) + texture uv + RGBA color. `uv.x < 0`
/// is the solid-color sentinel (see `ui.wgsl`). 32 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// uv sentinel marking a solid-color quad (no texture sample).
pub(super) const SOLID_UV: [f32; 2] = [-1.0, -1.0];

/// Opaque white tint: draw a textured quad at full color.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const TEXT_DISABLED: [f32; 4] = [0.62, 0.62, 0.62, 1.0];
const TEXT_PLACEHOLDER: [f32; 4] = [0.50, 0.56, 0.56, 1.0];
const INPUT_ACTIVE: [f32; 4] = [0.72, 0.67, 0.62, 1.0];
const INPUT_SELECTION: [f32; 4] = [0.34, 0.58, 0.96, 0.58];

/// Slot interior side (logical px) — the textured icon area. The drag cursor
/// sizes the held item to this; slot rects themselves come from the manifest.
pub(super) const SLOT_PX: f32 = 16.0;

/// Heart sprite side in logical px (the atlas cells are 9×9), scaled by `gui_scale`.
const HEART_PX: f32 = 9.0;
/// Horizontal step between hearts in logical px — one under the sprite width so
/// neighbours tuck together like the vanilla bar.
const HEART_STEP: f32 = 8.0;
/// Gap from the screen's left and bottom edges to the heart bar, logical px.
const HEART_MARGIN: f32 = 8.0;
/// One heart atlas cell as a fraction of atlas width — three cells: empty | half | full.
const HEART_CELL_U: f32 = 1.0 / 3.0;

/// One dynamic-overlay/widget quad group's place in [`UiBuild::overlays`]:
/// which sprite texture binds it (the interned image file name) and how many
/// vertices it spans.
#[derive(Copy, Clone, Debug)]
pub struct OverlaySpan {
    pub tex: SpriteKey,
    pub count: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextMode {
    /// Rarely-changing labels: rasterize the fitted whole run into the text atlas,
    /// then draw it as one quad.
    Rasterized,
    /// Editable text: draw one textured glyph quad per character from the dynamic
    /// glyph atlas.
    Glyphs,
}

#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub cell_px: f32,
    pub color: [f32; 4],
    pub shadow: bool,
}

/// The CPU-built UI for this frame, in paint order. Buffers are reused across
/// frames (cleared, capacity retained) per the no-per-frame-allocation rule.
#[derive(Default)]
pub struct UiBuild {
    /// Fullscreen dim backdrop behind an open menu (solid; empty for the HUD).
    pub dim: Vec<UiVertex>,
    /// The baked panel PNG quad (textured by its own panel bind group).
    pub panel: Vec<UiVertex>,
    /// Dynamic overlay quads (furnace gauges, mod gauges) and mod widget art
    /// concatenated; each group its own texture.
    pub overlays: Vec<UiVertex>,
    /// Per-group `(sprite key, vertex count)` describing how to slice + bind
    /// `overlays`.
    pub overlay_spans: Vec<OverlaySpan>,
    /// Hover / selection highlight quad (its own texture). Empty when nothing is
    /// highlighted.
    pub hover: Vec<UiVertex>,
    /// Builder-baked app-shell skin quad (title/world-select/create-world/pause).
    pub shell_skin: Vec<UiVertex>,
    /// Builder-baked dynamic app-shell scrollbar thumbs.
    pub shell_scroll_thumb: Vec<UiVertex>,
    /// HUD heart quads (bottom-left health bar), sampling the heart atlas. Empty for a
    /// spectator or behind an open menu.
    pub hearts: Vec<UiVertex>,
    /// Whole-run rasterized shell labels. The renderer packs these into a runtime
    /// text atlas and draws one textured quad per label.
    pub raster_text_runs: Vec<TextRun>,
    /// Editable shell text. The renderer draws these from a compiled glyph atlas.
    pub glyph_text_runs: Vec<TextRun>,
    /// One `(item, slot rect)` per filled slot. The renderer resolves each to its
    /// pre-baked icon-atlas cell and emits a textured quad.
    pub icon_quads: Vec<(ItemType, SlotRect)>,
    /// Like [`icon_quads`](Self::icon_quads) but drawn semi-transparent (greyed) — the
    /// furniture-workbench results the placed block can't yet make (not enough input).
    pub dim_icon_quads: Vec<(ItemType, SlotRect)>,
    /// Stack-count digits (solid), drawn over the icons.
    pub counts: Vec<UiVertex>,
    /// Cursor-held item icon, drawn front-most.
    pub drag_icon_quads: Vec<(ItemType, SlotRect)>,
    /// Cursor-held stack-count digits, drawn over the cursor icon.
    pub drag_counts: Vec<UiVertex>,
    /// Which baked GUI this frame draws, so the renderer binds the matching panel /
    /// hover / overlay textures. `None` when nothing is drawn.
    pub(crate) kind: Option<GuiKind>,
    /// Which baked app-shell skin this frame draws.
    pub(crate) shell_kind: Option<ShellKind>,
}

impl UiBuild {
    fn clear(&mut self) {
        self.dim.clear();
        self.panel.clear();
        self.overlays.clear();
        self.overlay_spans.clear();
        self.hover.clear();
        self.shell_skin.clear();
        self.shell_scroll_thumb.clear();
        self.hearts.clear();
        self.raster_text_runs.clear();
        self.glyph_text_runs.clear();
        self.icon_quads.clear();
        self.dim_icon_quads.clear();
        self.counts.clear();
        self.drag_icon_quads.clear();
        self.drag_counts.clear();
        self.kind = None;
        self.shell_kind = None;
    }
}

/// Convert a physical-pixel point (top-left origin, y down) to NDC (y up).
#[inline]
pub(super) fn pixel_to_ndc(screen: (u32, u32), x: f32, y: f32) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0]
}

/// Push a solid-color quad covering pixel rect `(x,y,w,h)`.
pub(super) fn push_solid(
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
pub(super) fn push_quad_uv(
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
    let p_tl = pixel_to_ndc(screen, x, y);
    let p_tr = pixel_to_ndc(screen, x + w, y);
    let p_br = pixel_to_ndc(screen, x + w, y + h);
    let p_bl = pixel_to_ndc(screen, x, y + h);
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

fn push_shell_box(out: &mut Vec<UiVertex>, screen: (u32, u32), r: SlotRect, color: [f32; 4]) {
    push_solid(out, screen, r.x, r.y, r.w, r.h, color);
}

fn push_shell_hover(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    r: SlotRect,
    margin: f32,
    opacity: f32,
    fit: HoverFit,
    image_size: (u32, u32),
    scale: f32,
) {
    let dest = SlotRect {
        x: r.x - margin,
        y: r.y - margin,
        w: r.w + 2.0 * margin,
        h: r.h + 2.0 * margin,
    };
    push_fit_textured(
        out,
        screen,
        dest,
        fit,
        image_size,
        scale,
        [1.0, 1.0, 1.0, opacity],
    );
}

fn push_fit_textured(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    r: SlotRect,
    fit: HoverFit,
    image_size: (u32, u32),
    scale: f32,
    color: [f32; 4],
) {
    let (sw, sh) = (image_size.0.max(1) as f32, image_size.1.max(1) as f32);
    match fit {
        HoverFit::Stretch => {
            push_quad_uv(
                out,
                screen,
                r.x,
                r.y,
                r.w,
                r.h,
                [0.0, 0.0],
                [1.0, 1.0],
                color,
            );
        }
        HoverFit::Tile => {
            let tile_w = (sw * scale.max(1.0)).max(1.0);
            let tile_h = (sh * scale.max(1.0)).max(1.0);
            let mut y = r.y;
            let y_end = r.y + r.h;
            while y < y_end - 0.01 {
                let h = tile_h.min(y_end - y);
                let mut x = r.x;
                let x_end = r.x + r.w;
                while x < x_end - 0.01 {
                    let w = tile_w.min(x_end - x);
                    push_quad_uv(
                        out,
                        screen,
                        x,
                        y,
                        w,
                        h,
                        [0.0, 0.0],
                        [w / tile_w, h / tile_h],
                        color,
                    );
                    x += tile_w;
                }
                y += tile_h;
            }
        }
        HoverFit::NineSlice {
            src_l,
            src_r,
            src_t,
            src_b,
            dst_l,
            dst_r,
            dst_t,
            dst_b,
        } => {
            let src_l = src_l.clamp(0.0, sw * 0.5);
            let src_r = src_r.clamp(0.0, sw * 0.5);
            let src_t = src_t.clamp(0.0, sh * 0.5);
            let src_b = src_b.clamp(0.0, sh * 0.5);
            let dst_l = (dst_l * scale).clamp(0.0, r.w * 0.5);
            let dst_r = (dst_r * scale).clamp(0.0, r.w * 0.5);
            let dst_t = (dst_t * scale).clamp(0.0, r.h * 0.5);
            let dst_b = (dst_b * scale).clamp(0.0, r.h * 0.5);

            let xs = [0.0, src_l, sw - src_r, sw];
            let ys = [0.0, src_t, sh - src_b, sh];
            let dx = [r.x, r.x + dst_l, r.x + r.w - dst_r, r.x + r.w];
            let dy = [r.y, r.y + dst_t, r.y + r.h - dst_b, r.y + r.h];
            for row in 0..3 {
                for col in 0..3 {
                    let w = dx[col + 1] - dx[col];
                    let h = dy[row + 1] - dy[row];
                    if w <= 0.01 || h <= 0.01 {
                        continue;
                    }
                    push_quad_uv(
                        out,
                        screen,
                        dx[col],
                        dy[row],
                        w,
                        h,
                        [xs[col] / sw, ys[row] / sh],
                        [xs[col + 1] / sw, ys[row + 1] / sh],
                        color,
                    );
                }
            }
        }
    }
}

fn inset_rect(r: SlotRect, inset: f32) -> SlotRect {
    SlotRect {
        x: r.x + inset,
        y: r.y + inset,
        w: (r.w - inset * 2.0).max(0.0),
        h: (r.h - inset * 2.0).max(0.0),
    }
}

fn framed_text_rect(r: SlotRect, scale: f32) -> SlotRect {
    let inset = (6.0 * scale).max(4.0);
    inset_rect(r, inset)
}

fn push_shell_text_run(build: &mut UiBuild, text: &ShellText, mode: TextMode, shadow: bool) {
    let cell = text.cell_px.max(1.0);
    let fitted = fit_text(&text.text, text.rect.w, cell);
    if fitted.is_empty() {
        return;
    }
    let width = crate::render::ui_text::text_width(&fitted) as f32 * cell;
    let height = crate::render::ui_text::TEXT_GLYPH_H as f32 * cell;
    let x = match text.align {
        ShellTextAlign::Left => text.rect.x,
        ShellTextAlign::Center => text.rect.x + (text.rect.w - width).max(0.0) * 0.5,
    };
    let y = text.rect.y + (text.rect.h - height).max(0.0) * 0.5;
    let run = TextRun {
        text: fitted,
        x,
        y,
        cell_px: cell,
        color: text.color,
        shadow,
    };
    match mode {
        TextMode::Rasterized => build.raster_text_runs.push(run),
        TextMode::Glyphs => build.glyph_text_runs.push(run),
    }
}

fn fit_text(text: &str, max_width: f32, cell: f32) -> String {
    let width = |s: &str| crate::render::ui_text::text_width(s) as f32 * cell;
    if width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis = "...";
    if width(ellipsis) > max_width {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut candidate = out.clone();
        candidate.push(ch);
        candidate.push_str(ellipsis);
        if width(&candidate) > max_width {
            break;
        }
        out.push(ch);
    }
    out.push_str(ellipsis);
    out
}

fn push_button(build: &mut UiBuild, screen: (u32, u32), button: &ShellButton, scale: f32) {
    if !button.enabled {
        push_shell_box(
            &mut build.counts,
            screen,
            button.rect,
            [0.0, 0.0, 0.0, 0.36],
        );
    }
    let text = ShellText {
        rect: framed_text_rect(button.rect, scale),
        text: button.label.clone(),
        color: if button.enabled { WHITE } else { TEXT_DISABLED },
        cell_px: scale.max(1.0),
        align: ShellTextAlign::Center,
    };
    push_shell_text_run(build, &text, TextMode::Rasterized, true);
}

fn push_input(build: &mut UiBuild, screen: (u32, u32), input: &ShellInput, scale: f32) {
    let cell = scale.max(1.0);
    if input.active {
        let line = cell;
        push_solid(
            &mut build.counts,
            screen,
            input.rect.x + line,
            input.rect.y + line,
            (input.rect.w - line * 2.0).max(0.0),
            line,
            INPUT_ACTIVE,
        );
    }
    let text = if input.text.is_empty() {
        input.placeholder.as_str()
    } else {
        input.text.as_str()
    };
    let text_rect = gui_layout::shell_input_text_rect(input.rect, scale);
    if input.active {
        let glyph_h = crate::render::ui_text::TEXT_GLYPH_H as f32 * cell;
        let y = text_rect.y + (text_rect.h - glyph_h).max(0.0) * 0.5;
        if let Some((start, end)) = input.selection {
            let start = start.min(end);
            let end = end.max(start);
            if end > start {
                let advance = crate::render::ui_text::TEXT_GLYPH_ADVANCE as f32 * cell;
                push_solid(
                    &mut build.counts,
                    screen,
                    text_rect.x + start as f32 * advance,
                    y - cell,
                    (end - start) as f32 * advance,
                    glyph_h + cell * 2.0,
                    INPUT_SELECTION,
                );
            }
        }
        if input.show_cursor {
            let advance = crate::render::ui_text::TEXT_GLYPH_ADVANCE as f32 * cell;
            let cursor_w = cell.max(1.0);
            push_solid(
                &mut build.counts,
                screen,
                text_rect.x + input.cursor as f32 * advance,
                y,
                cursor_w,
                glyph_h,
                WHITE,
            );
        }
    }
    let text = ShellText {
        rect: text_rect,
        text: text.to_string(),
        color: if input.text.is_empty() {
            TEXT_PLACEHOLDER
        } else {
            WHITE
        },
        cell_px: cell,
        align: ShellTextAlign::Left,
    };
    push_shell_text_run(build, &text, TextMode::Glyphs, true);
}

fn push_row(build: &mut UiBuild, screen: (u32, u32), row: &ShellListRow, scale: f32) {
    if row.selected {
        push_shell_box(&mut build.counts, screen, row.rect, [0.75, 0.95, 1.0, 0.18]);
    }
    let text = ShellText {
        rect: SlotRect {
            x: row.rect.x + 7.0 * scale,
            y: row.rect.y,
            w: (row.rect.w - 14.0 * scale).max(0.0),
            h: row.rect.h,
        },
        text: row.label.clone(),
        color: WHITE,
        cell_px: scale.max(1.0),
        align: ShellTextAlign::Left,
    };
    push_shell_text_run(build, &text, TextMode::Rasterized, true);
}

fn push_scrollbar_thumb(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    scrollbar: &ShellScrollbar,
    fit: HoverFit,
    image_size: (u32, u32),
    scale: f32,
) {
    let r = scrollbar.thumb;
    push_fit_textured(out, screen, r, fit, image_size, scale, WHITE);
}

fn push_shell_ui(ui: &UiSnapshot, build: &mut UiBuild, scale: f32) {
    if !ui.shell.active {
        return;
    }
    let screen = ui.screen;
    for quad in &ui.shell.quads {
        push_shell_box(&mut build.dim, screen, quad.rect, quad.color);
    }
    let kind = ui
        .shell
        .skin
        .expect("active shell UI must name a baked shell skin");
    let def = gui_layout::shell_def(kind).unwrap_or_else(|| {
        panic!(
            "missing required baked shell GUI {:?}; bake it to assets/textures/gui/shell/baked",
            kind
        )
    });
    build.shell_kind = Some(kind);
    let r = def.panel_rect(screen);
    push_quad_uv(
        &mut build.shell_skin,
        screen,
        r.x,
        r.y,
        r.w,
        r.h,
        [0.0, 0.0],
        [1.0, 1.0],
        WHITE,
    );
    if let Some(thumb) = def.scroll_thumb() {
        for scrollbar in &ui.shell.scrollbars {
            push_scrollbar_thumb(
                &mut build.shell_scroll_thumb,
                screen,
                scrollbar,
                thumb.fit,
                thumb.image_size,
                scale,
            );
        }
    }
    if let Some(hover) = def.hover() {
        let margin = hover.margin as f32 * scale;
        for row in &ui.shell.rows {
            if row.hovered {
                push_shell_hover(
                    &mut build.hover,
                    screen,
                    row.rect,
                    margin,
                    hover.opacity,
                    hover.fit,
                    hover.image_size,
                    scale,
                );
            }
        }
        for button in &ui.shell.buttons {
            if button.enabled && button.hovered {
                push_shell_hover(
                    &mut build.hover,
                    screen,
                    button.rect,
                    margin,
                    hover.opacity,
                    hover.fit,
                    hover.image_size,
                    scale,
                );
            }
        }
    }
    for row in &ui.shell.rows {
        push_row(build, screen, row, scale);
    }
    for input in &ui.shell.inputs {
        push_input(build, screen, input, scale);
    }
    for button in &ui.shell.buttons {
        push_button(build, screen, button, scale);
    }
    for text in &ui.shell.texts {
        push_shell_text_run(build, text, TextMode::Rasterized, true);
    }
}

/// The game state filling slot `i` of `role`, as `(item, count)` — the one place
/// that maps a manifest role to its backing model. `None` for an empty slot or a
/// decorative role.
fn slot_item(ui: &UiSnapshot, role: Role, i: usize) -> Option<(ItemType, u32)> {
    let stack = |s: crate::item::ItemStack| (s.item, s.count as u32);
    match role {
        Role::Storage => ui
            .chest
            .and_then(|c| c.slots.get(i).copied().flatten())
            .map(stack),
        Role::Hotbar => ui
            .slots
            .get(i)
            .copied()
            .flatten()
            .map(|(it, c)| (it, c as u32)),
        Role::PlayerInv => ui
            .slots
            .get(HOTBAR_LEN + i)
            .copied()
            .flatten()
            .map(|(it, c)| (it, c as u32)),
        Role::CraftInput => ui
            .craft
            .get(i)
            .copied()
            .flatten()
            .map(|(it, c)| (it, c as u32)),
        Role::CraftResult => ui.result.map(|(it, c)| (it, c as u32)),
        Role::FurnaceInput => ui.furnace.and_then(|f| f.input).map(stack),
        Role::FurnaceFuel => ui.furnace.and_then(|f| f.fuel).map(stack),
        Role::FurnaceOutput => ui.furnace.and_then(|f| f.output).map(stack),
        Role::WorkbenchInput => ui.workbench.as_ref().and_then(|w| w.input).map(stack),
        // The result grid carries a craftable flag (greyed when not), so it's drawn
        // specially in `build_ui` rather than through this (item, count) mapping.
        Role::WorkbenchResult => None,
        Role::Generic | Role::Other => None,
    }
}

/// Record the sprite span for vertices pushed into `build.overlays` since
/// `start`, so the renderer binds `tex` for exactly that quad group.
fn close_sprite_span(build: &mut UiBuild, start: usize, tex: SpriteKey) {
    let count = (build.overlays.len() - start) as u32;
    if count > 0 {
        build.overlay_spans.push(OverlaySpan { tex, count });
    }
}

/// Emit a dynamic overlay clipped by `frac` (`0..=1`), recording its sprite
/// span so the renderer binds the overlay's own texture. No-op at `frac <= 0`.
/// The fill direction is the overlay row's declared mode: `grow_lr` grows
/// left→right with the fraction (the smelt arrow); `deplete_td` keeps the
/// bottom `frac` visible (the burn flame as fuel runs out).
fn push_overlay(
    build: &mut UiBuild,
    screen: (u32, u32),
    r: SlotRect,
    mode: OverlayMode,
    tex: SpriteKey,
    frac: f32,
) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let start = build.overlays.len();
    match mode {
        OverlayMode::GrowLr => {
            push_quad_uv(
                &mut build.overlays,
                screen,
                r.x,
                r.y,
                r.w * frac,
                r.h,
                [0.0, 0.0],
                [frac, 1.0],
                WHITE,
            );
        }
        OverlayMode::DepleteTd => {
            let lit_h = r.h * frac;
            let top = r.h - lit_h;
            push_quad_uv(
                &mut build.overlays,
                screen,
                r.x,
                r.y + top,
                r.w,
                lit_h,
                [0.0, 1.0 - frac],
                [1.0, 1.0],
                WHITE,
            );
        }
    }
    close_sprite_span(build, start, tex);
}

/// An overlay's `0..=1` fraction: the furnace tags read the live
/// [`FurnaceView`](crate::gui::FurnaceView) (KEEPING the furnace gauges'
/// behaviour identical to the pre-Phase-5 hardcode); anything else reads the
/// GUI state map at the tag key (`F32`; absent = 0, drawn not at all).
fn overlay_fraction(ui: &UiSnapshot, tag: &str) -> f32 {
    if let Some(f) = ui.furnace {
        match tag {
            "furnace_arrow" => return f.cook01,
            "furnace_flame" => return f.burn01,
            _ => {}
        }
    }
    match state_value(ui, tag) {
        Some(GuiValue::F32(v)) => *v,
        _ => 0.0,
    }
}

/// A GUI state map read for this frame's snapshot, or `None` when no mod GUI
/// session is up / the key is absent.
fn state_value<'a>(ui: &'a UiSnapshot, key: &str) -> Option<&'a GuiValue> {
    ui.gui_state.as_ref()?.get(key)
}

/// A state value rendered as label text.
fn state_text(v: &GuiValue) -> String {
    match v {
        GuiValue::Str(s) => s.clone(),
        GuiValue::I32(i) => i.to_string(),
        GuiValue::F32(f) => format!("{f}"),
    }
}

/// The four pixel-space corners of `r` rotated by `angle` radians (clockwise
/// on screen, y-down) around `pivot` (absolute pixels), in tl/tr/br/bl order.
fn rotated_quad_corners(r: SlotRect, pivot: (f32, f32), angle: f32) -> [[f32; 2]; 4] {
    let (sin, cos) = angle.sin_cos();
    let rot = |x: f32, y: f32| -> [f32; 2] {
        let (dx, dy) = (x - pivot.0, y - pivot.1);
        [pivot.0 + dx * cos - dy * sin, pivot.1 + dx * sin + dy * cos]
    };
    [
        rot(r.x, r.y),
        rot(r.x + r.w, r.y),
        rot(r.x + r.w, r.y + r.h),
        rot(r.x, r.y + r.h),
    ]
}

/// Push a full-texture quad whose corners are arbitrary pixel-space points
/// (tl/tr/br/bl) — the rotated `rotimage` draw. Same two-triangle winding as
/// [`push_quad_uv`].
fn push_corner_quad(out: &mut Vec<UiVertex>, screen: (u32, u32), c: [[f32; 2]; 4]) {
    let p = |i: usize| pixel_to_ndc(screen, c[i][0], c[i][1]);
    let v = |pos: [f32; 2], uv: [f32; 2]| UiVertex {
        pos,
        uv,
        color: WHITE,
    };
    let (tl, tr, br, bl) = (p(0), p(1), p(2), p(3));
    out.push(v(tl, [0.0, 0.0]));
    out.push(v(bl, [0.0, 1.0]));
    out.push(v(br, [1.0, 1.0]));
    out.push(v(tl, [0.0, 0.0]));
    out.push(v(br, [1.0, 1.0]));
    out.push(v(tr, [1.0, 0.0]));
}

/// Draw a mod GUI's widgets: static images and button art into the sprite
/// spans, `rotimage` quads rotated by their state angle, the button hover
/// (its own `hover_image`, or the GUI's shared hover highlight), and labels
/// through the runtime text pipeline (static `text` overridden by the state
/// map at `state_key`).
fn push_widgets(
    ui: &UiSnapshot,
    build: &mut UiBuild,
    def: &'static gui_layout::GuiDef,
    scale: f32,
) {
    let screen = ui.screen;
    def.for_each_widget(screen, |w, r| match w {
        WidgetDef::Image { image, .. } => {
            let start = build.overlays.len();
            push_quad_uv(
                &mut build.overlays,
                screen,
                r.x,
                r.y,
                r.w,
                r.h,
                [0.0, 0.0],
                [1.0, 1.0],
                WHITE,
            );
            close_sprite_span(build, start, image);
        }
        WidgetDef::Button {
            image, hover_image, ..
        } => {
            let hovered = r.contains(ui.cursor_px.0, ui.cursor_px.1);
            if let Some(tex) = image {
                let start = build.overlays.len();
                push_quad_uv(
                    &mut build.overlays,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [0.0, 0.0],
                    [1.0, 1.0],
                    WHITE,
                );
                close_sprite_span(build, start, tex);
            }
            if hovered {
                if let Some(tex) = hover_image {
                    // A dedicated hover skin replaces the shared highlight.
                    let start = build.overlays.len();
                    push_quad_uv(
                        &mut build.overlays,
                        screen,
                        r.x,
                        r.y,
                        r.w,
                        r.h,
                        [0.0, 0.0],
                        [1.0, 1.0],
                        WHITE,
                    );
                    close_sprite_span(build, start, tex);
                } else if let Some(h) = def.hover() {
                    push_shell_hover(
                        &mut build.hover,
                        screen,
                        r,
                        h.margin as f32 * scale,
                        h.opacity,
                        h.fit,
                        h.image_size,
                        scale,
                    );
                }
            }
        }
        WidgetDef::Rotimage {
            image,
            pivot,
            state_key,
            ..
        } => {
            let angle = match state_value(ui, state_key) {
                Some(GuiValue::F32(a)) => *a,
                _ => 0.0,
            };
            let pivot = match pivot {
                Some([px, py]) => (r.x + px * scale, r.y + py * scale),
                None => (r.x + r.w * 0.5, r.y + r.h * 0.5),
            };
            let start = build.overlays.len();
            push_corner_quad(
                &mut build.overlays,
                screen,
                rotated_quad_corners(r, pivot, angle),
            );
            close_sprite_span(build, start, image);
        }
        WidgetDef::Label {
            text,
            state_key,
            align,
            color,
            ..
        } => {
            let dynamic = state_key
                .as_ref()
                .and_then(|k| state_value(ui, k))
                .map(state_text);
            let Some(content) = dynamic.or_else(|| text.clone()) else {
                return;
            };
            let run = ShellText {
                rect: r,
                text: content,
                color: *color,
                cell_px: scale.max(1.0),
                align: match align {
                    gui_layout::LabelAlign::Center => ShellTextAlign::Center,
                    gui_layout::LabelAlign::Left => ShellTextAlign::Left,
                },
            };
            push_shell_text_run(build, &run, TextMode::Rasterized, true);
        }
    });
}

/// Emit the bottom-left heart bar for `health` (half-heart points). Every heart gets an
/// empty container, then a full or half heart laid over it per the current health, so a
/// damaged heart shows the container through its missing portion — the vanilla read.
/// Scaled with the rest of the HUD. Called only for the [`GuiKind::Hotbar`] HUD.
fn push_hearts(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    health: crate::gui::HealthView,
    scale: f32,
) {
    let hearts = (health.max / 2).max(0);
    if hearts == 0 {
        return;
    }
    let size = HEART_PX * scale;
    let step = HEART_STEP * scale;
    let margin = HEART_MARGIN * scale;
    let y = screen.1 as f32 - margin - size;
    // Atlas cell `c` (0 empty, 1 half, 2 full) as top-left / bottom-right uv corners.
    let cell_uv = |c: i32| -> ([f32; 2], [f32; 2]) {
        let u0 = c as f32 * HEART_CELL_U;
        ([u0, 0.0], [u0 + HEART_CELL_U, 1.0])
    };
    let current = health.current.clamp(0, health.max);
    for i in 0..hearts {
        let x = margin + i as f32 * step;
        let (tl, br) = cell_uv(0);
        push_quad_uv(out, screen, x, y, size, size, tl, br, WHITE);
        // 2 points = full heart, 1 = half, 0 = just the empty container.
        let cell = match (current - i * 2).clamp(0, 2) {
            2 => 2,
            1 => 1,
            _ => continue,
        };
        let (tl, br) = cell_uv(cell);
        push_quad_uv(out, screen, x, y, size, size, tl, br, WHITE);
    }
}

/// Build the full UI for `ui` this frame from its open [`GuiKind`]'s baked def.
/// The buffers are the caller-owned reusable [`UiBuild`] (cleared, capacity kept).
pub fn build_ui(ui: &UiSnapshot, build: &mut UiBuild) {
    build.clear();

    let screen = ui.screen;
    if screen.0 == 0 || screen.1 == 0 {
        return;
    }
    let scale = gui_scale(screen);
    let kind = ui.kind;
    let Some(def) = gui_layout::def(kind) else {
        push_shell_ui(ui, build, scale);
        return; // No baked manifest for this screen => nothing to draw.
    };
    build.kind = Some(kind);

    // Dim the screen behind an open menu (the HUD hotbar has no backdrop).
    if ui.open {
        push_solid(
            &mut build.dim,
            screen,
            0.0,
            0.0,
            screen.0 as f32,
            screen.1 as f32,
            [0.0, 0.0, 0.0, 0.6],
        );
    }

    // Panel art.
    let pr = def.panel_rect(screen);
    push_quad_uv(
        &mut build.panel,
        screen,
        pr.x,
        pr.y,
        pr.w,
        pr.h,
        [0.0, 0.0],
        [1.0, 1.0],
        WHITE,
    );

    // Dynamic overlays, in manifest order: the furnace's smelt arrow + burn
    // flame (fractions from FurnaceView, exactly as before) and mod overlays
    // (fractions from the GUI state map at the tag key).
    def.for_each_overlay(screen, |o, r| {
        push_overlay(
            build,
            screen,
            r,
            o.mode,
            o.image,
            overlay_fraction(ui, o.tag),
        );
    });

    // Mod GUI widgets (images, buttons + their hover, rotimages, labels).
    push_widgets(ui, build, def, scale);

    // Hover / selection highlight. The hotbar HUD always highlights the active
    // slot (the held item); every open menu highlights the slot under the cursor.
    if let Some(h) = def.hover() {
        let highlighted = if kind == GuiKind::Hotbar {
            def.role_rect(Role::Hotbar, ui.active as usize, screen)
        } else {
            def.hovered_slot_rect(screen, ui.cursor_px)
        };
        if let Some(sr) = highlighted {
            let m = h.margin as f32 * scale;
            push_fit_textured(
                &mut build.hover,
                screen,
                SlotRect {
                    x: sr.x - m,
                    y: sr.y - m,
                    w: sr.w + 2.0 * m,
                    h: sr.h + 2.0 * m,
                },
                h.fit,
                h.image_size,
                scale,
                [1.0, 1.0, 1.0, h.opacity],
            );
        }
    }

    // HUD hearts (bottom-left). Only on the hotbar HUD, so an open menu hides them —
    // and a spectator carries no health, so `None` draws nothing.
    if kind == GuiKind::Hotbar {
        if let Some(health) = ui.health {
            push_hearts(&mut build.hearts, screen, health, scale);
        }
    }

    // Every filled slot's item icon + stack count.
    def.for_each_slot(screen, |role, i, r| {
        // The workbench result grid is a list of offered recipes, each greyed when the
        // placed block isn't yet enough to craft it — drawn here (no stack count).
        if role == Role::WorkbenchResult {
            if let Some(&(item, craftable)) = ui.workbench.as_ref().and_then(|w| w.results.get(i)) {
                if item != ItemType::Air {
                    if craftable {
                        icon::push_slot_icon(build, screen, item, r);
                    } else {
                        icon::push_dim_slot_icon(build, screen, item, r);
                    }
                }
            }
            return;
        }
        let Some((item, count)) = slot_item(ui, role, i) else {
            return;
        };
        if item == ItemType::Air || count == 0 {
            return;
        }
        icon::push_slot_icon(build, screen, item, r);
        if count > 1 {
            icon::push_count(&mut build.counts, screen, count, r, scale);
        }
    });

    // Cursor-held stack (drag/drop), front-most — only with a menu open.
    if ui.open {
        if let Some((item, count)) = ui.cursor {
            if item != ItemType::Air && count > 0 {
                let s = SLOT_PX * scale;
                let (cx, cy) = ui.cursor_px;
                let r = SlotRect {
                    x: cx - s * 0.5,
                    y: cy - s * 0.5,
                    w: s,
                    h: s,
                };
                build.drag_icon_quads.push((item, r));
                if count > 1 {
                    icon::push_count(&mut build.drag_counts, screen, count as u32, r, scale);
                }
            }
        }
    }

    push_shell_ui(ui, build, scale);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(kind: GuiKind, open: bool) -> UiSnapshot {
        let mut s = UiSnapshot {
            kind,
            open,
            screen: (1280, 720),
            cursor_px: (640.0, 360.0),
            active: 2,
            ..Default::default()
        };
        s.slots[0] = Some((ItemType::Stone, 64));
        s
    }

    #[test]
    fn gui_scale_is_clamped_and_increases_with_height() {
        assert_eq!(gui_scale((320, 240)), 1.0);
        assert!(gui_scale((1920, 1080)) >= 2.0);
        assert_eq!(gui_scale((10, 10)), 1.0);
        assert_eq!(gui_scale((10000, 10000)), 4.0);
    }

    #[test]
    fn zero_screen_builds_nothing() {
        let mut b = UiBuild::default();
        let s = UiSnapshot {
            screen: (0, 0),
            ..snap(GuiKind::Hotbar, false)
        };
        build_ui(&s, &mut b);
        assert!(b.panel.is_empty() && b.icon_quads.is_empty() && b.kind.is_none());
    }

    #[test]
    fn hotbar_hud_draws_panel_active_selection_and_item() {
        // Uses the real baked hotbar manifest (the contract guard ensures it loads).
        let mut b = UiBuild::default();
        build_ui(&snap(GuiKind::Hotbar, false), &mut b);
        assert_eq!(b.kind, Some(GuiKind::Hotbar));
        assert!(!b.panel.is_empty(), "hotbar panel drawn");
        assert!(!b.hover.is_empty(), "active-slot selection drawn");
        assert!(b.dim.is_empty(), "HUD has no dim backdrop");
        assert_eq!(b.icon_quads.len(), 1, "the one hotbar item");
    }

    #[test]
    fn open_menu_dims_and_draws_drag_cursor() {
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Inventory, true);
        s.cursor = Some((ItemType::Dirt, 12));
        build_ui(&s, &mut b);
        assert!(!b.dim.is_empty(), "menu dims the screen");
        assert!(!b.panel.is_empty());
        assert_eq!(b.drag_icon_quads.len(), 1);
        assert!(!b.drag_counts.is_empty(), "drag count > 1 drawn");
    }

    #[test]
    fn world_select_shell_text_uses_runtime_raster_runs() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        let def = gui_layout::shell_def(ShellKind::WorldSelect).expect("world select shell def");
        let panel = def.panel_rect(screen);
        let long_name = "A Very Long Saved World Name That Should Clip In The Row";
        let mut shell = crate::gui::ShellUiSnapshot {
            active: true,
            skin: Some(ShellKind::WorldSelect),
            ..Default::default()
        };
        shell.texts.push(ShellText {
            rect: SlotRect {
                x: panel.x,
                y: panel.y + 10.0 * scale,
                w: panel.w,
                h: 12.0 * scale,
            },
            text: "Select World".to_string(),
            color: WHITE,
            cell_px: 1.3 * scale,
            align: ShellTextAlign::Center,
        });
        for (i, rect) in def
            .role_rects(crate::gui::ShellRole::WorldRow, screen)
            .iter()
            .copied()
            .enumerate()
        {
            shell.rows.push(ShellListRow {
                rect,
                label: format!("{long_name} {i}"),
                selected: i == 0,
                hovered: false,
            });
        }
        for (role, label, enabled) in [
            (crate::gui::ShellRole::WorldPlay, "Play", true),
            (crate::gui::ShellRole::WorldCreate, "Create New World", true),
            (crate::gui::ShellRole::WorldDelete, "Delete World", true),
            (crate::gui::ShellRole::WorldBack, "Back", true),
        ] {
            if let Some(rect) = def.role_rect(role, screen) {
                shell.buttons.push(ShellButton {
                    rect,
                    label: label.to_string(),
                    enabled,
                    hovered: false,
                });
            }
        }

        let mut b = UiBuild::default();
        build_ui(
            &UiSnapshot {
                kind: GuiKind::Other,
                screen,
                shell,
                ..Default::default()
            },
            &mut b,
        );

        let cap = crate::render::pipeline::MAX_UI_VERTICES as usize;
        assert_eq!(b.shell_kind, Some(ShellKind::WorldSelect));
        assert!(!b.shell_skin.is_empty(), "world-select skin drawn");
        assert_eq!(
            b.raster_text_runs.len(),
            10,
            "title, five rows, and four buttons are whole-run text"
        );
        assert!(
            b.glyph_text_runs.is_empty(),
            "world-select has no editable text"
        );
        assert!(
            b.counts.len() <= cap,
            "solid UI vertices should no longer include shell text cells: {} > {cap}",
            b.counts.len()
        );
    }

    #[test]
    fn shell_inputs_use_dynamic_glyph_runs() {
        let screen = (1280, 720);
        let mut shell = crate::gui::ShellUiSnapshot {
            active: true,
            skin: Some(ShellKind::CreateWorld),
            ..Default::default()
        };
        shell.inputs.push(ShellInput {
            rect: SlotRect {
                x: 320.0,
                y: 240.0,
                w: 240.0,
                h: 32.0,
            },
            text: "World".to_string(),
            placeholder: "My World".to_string(),
            active: true,
            cursor: 5,
            selection: None,
            show_cursor: true,
        });

        let mut b = UiBuild::default();
        build_ui(
            &UiSnapshot {
                kind: GuiKind::Other,
                screen,
                shell,
                ..Default::default()
            },
            &mut b,
        );

        assert_eq!(b.glyph_text_runs.len(), 1);
        assert_eq!(b.glyph_text_runs[0].text, "World");
        assert!(b.raster_text_runs.is_empty());
        assert!(!b.counts.is_empty(), "active input underline stays solid");
    }

    #[test]
    fn hotbar_hud_draws_hearts_from_health() {
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Hotbar, false);
        s.health = Some(crate::gui::HealthView {
            current: 15,
            max: 20,
        });
        build_ui(&s, &mut b);
        assert!(!b.hearts.is_empty(), "the HUD draws the heart bar");
    }

    #[test]
    fn hearts_hidden_behind_a_menu_and_without_health() {
        // Behind an open inventory (kind != Hotbar) hearts are hidden even with health.
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Inventory, true);
        s.health = Some(crate::gui::HealthView {
            current: 20,
            max: 20,
        });
        build_ui(&s, &mut b);
        assert!(b.hearts.is_empty(), "no hearts behind an open menu");
        // On the HUD but with no health (a spectator): still nothing.
        let mut b2 = UiBuild::default();
        build_ui(&snap(GuiKind::Hotbar, false), &mut b2); // health defaults to None
        assert!(b2.hearts.is_empty(), "no hearts without survival health");
    }

    #[test]
    fn hover_nine_slice_emits_all_regions() {
        let mut verts = Vec::new();
        push_fit_textured(
            &mut verts,
            (1280, 720),
            SlotRect {
                x: 100.0,
                y: 100.0,
                w: 32.0,
                h: 32.0,
            },
            HoverFit::NineSlice {
                src_l: 4.0,
                src_r: 4.0,
                src_t: 4.0,
                src_b: 4.0,
                dst_l: 4.0,
                dst_r: 4.0,
                dst_t: 4.0,
                dst_b: 4.0,
            },
            (16, 16),
            1.0,
            WHITE,
        );
        assert_eq!(verts.len(), 9 * 6);
    }

    #[test]
    fn rotimage_corners_rotate_around_the_pivot() {
        let r = SlotRect {
            x: 10.0,
            y: 20.0,
            w: 8.0,
            h: 4.0,
        };
        let pivot = (14.0, 22.0); // rect centre
                                  // No angle = the axis-aligned rect.
        let c0 = rotated_quad_corners(r, pivot, 0.0);
        assert_eq!(c0[0], [10.0, 20.0]);
        assert_eq!(c0[2], [18.0, 24.0]);
        // A quarter turn (y-down screen space) maps the top-left corner's
        // offset (-4, -2) to (2, -4): rotation about the pivot, not the origin.
        let c90 = rotated_quad_corners(r, pivot, std::f32::consts::FRAC_PI_2);
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(close(c90[0], [16.0, 18.0]), "{:?}", c90[0]);
        assert!(close(c90[2], [12.0, 26.0]), "{:?}", c90[2]);
        // A full turn returns every corner home.
        let c360 = rotated_quad_corners(r, pivot, std::f32::consts::TAU);
        for (a, b) in c360.iter().zip(c0.iter()) {
            assert!(close(*a, *b));
        }
        // The pivot itself is a fixed point wherever it sits.
        let cp = rotated_quad_corners(r, (10.0, 20.0), 1.234);
        assert!(
            close(cp[0], [10.0, 20.0]),
            "corner at the pivot never moves"
        );
    }

    #[test]
    fn build_reuses_buffers_without_growth() {
        let mut b = UiBuild::default();
        build_ui(&snap(GuiKind::Inventory, true), &mut b);
        let cap = b.icon_quads.capacity();
        assert!(cap > 0);
        build_ui(&snap(GuiKind::Hotbar, false), &mut b);
        assert_eq!(b.icon_quads.capacity(), cap, "icon-quad buffer reused");
    }
}
