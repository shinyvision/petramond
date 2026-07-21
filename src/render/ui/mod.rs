//! UI / HUD geometry: turns the renderer's [`UiSnapshot`] into the per-frame
//! vertex buffers for the GAME-OWNED content of a document-drawn screen.
//!
//! Every screen's chrome (panels, slot faces, hover, gauges, text, dim) is
//! drawn by the GUI-document runtime (`petramond-ui` draw list → `renderer::doc_ui`).
//! [`build_ui`] emits only what the game owns on top of that: item icons +
//! stack counts in the document's solved slot cells,
//! the HUD hearts, and the cursor-held drag stack. When no document backs the
//! frame has no solved document slots, nothing draws.
//!
//! All layout math is in **physical pixels** (origin top-left, y down) and
//! converted to NDC only when emitting vertices.

// `pub(crate)` so the one-time icon-atlas bake (`renderer::icon_atlas`) can reach
// the `pub(crate)` MVP projection fns; the per-slot helpers stay `pub(super)`.
pub(crate) mod icon;

use crate::gui::{GuiKind, Role, SlotRect, UiSnapshot};
use crate::inventory::HOTBAR_LEN;
use crate::item::ItemType;
use petramond_text::tiny as tiny_text;

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

/// Slot interior side (logical px) — the textured icon area. The drag cursor
/// sizes the held item to this; slot rects themselves come from the document.
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

/// The CPU-built UI for this frame, in paint order. Buffers are reused across
/// frames (cleared, capacity retained) per the no-per-frame-allocation rule.
#[derive(Default)]
pub struct UiBuild {
    /// HUD heart quads (bottom-left health bar), sampling the heart atlas. Empty for a
    /// spectator or behind an open menu.
    pub hearts: Vec<UiVertex>,
    /// One `(item, slot rect)` per filled slot. The renderer resolves each to its
    /// pre-baked icon-atlas cell and emits a textured quad.
    pub icon_quads: Vec<(ItemType, SlotRect)>,
    /// Recipe-browser icons embedded in document hooks. Each carries the
    /// inherited scroll clip and whether the unavailable row should dim it.
    pub hook_icon_quads: Vec<HookIconQuad>,
    /// Stack-count digits (solid), drawn over the icons.
    pub counts: Vec<UiVertex>,
    /// Cursor-held item icon, drawn front-most.
    pub drag_icon_quads: Vec<(ItemType, SlotRect)>,
    /// Cursor-held stack-count digits, drawn over the cursor icon.
    pub drag_counts: Vec<UiVertex>,
    /// The hurt-flash edge vignette (solid, per-vertex alpha gradient), drawn
    /// under everything else in the UI pass. Empty on a calm frame.
    pub vignette: Vec<UiVertex>,
    /// HUD status-effect icons (framed row above the hearts), sampling the
    /// composed effect strip. Empty when nothing is active or behind a menu.
    pub effects: Vec<UiVertex>,
}

impl UiBuild {
    fn clear(&mut self) {
        self.hearts.clear();
        self.effects.clear();
        self.icon_quads.clear();
        self.hook_icon_quads.clear();
        self.counts.clear();
        self.drag_icon_quads.clear();
        self.drag_counts.clear();
        self.vignette.clear();
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HookIconQuad {
    pub item: ItemType,
    pub rect: SlotRect,
    pub clip: Option<SlotRect>,
    pub dim: bool,
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

/// The game-owned content of a document-drawn screen: item icons + counts in
/// the document's slot cells (inset one logical px so icons sit inside the
/// themed cell border), HUD hearts, and the
/// cursor-held drag stack. All chrome (panels, slot faces, hover, gauges,
/// text) is in the document draw list.
fn push_doc_game_content(
    ui: &UiSnapshot,
    build: &mut UiBuild,
    slots: &[crate::gui::DocSlot],
    screen: (u32, u32),
    scale: f32,
) {
    for slot in slots {
        let inset = scale;
        let r = SlotRect {
            x: slot.rect.x + inset,
            y: slot.rect.y + inset,
            w: (slot.rect.w - 2.0 * inset).max(0.0),
            h: (slot.rect.h - 2.0 * inset).max(0.0),
        };
        let i = slot.index as usize;
        let Some((item, count)) = slot_item(ui, slot.role, i) else {
            continue;
        };
        if item == ItemType::Air || count == 0 {
            continue;
        }
        icon::push_slot_icon(build, screen, item, r);
        if count > 1 {
            icon::push_count(&mut build.counts, screen, count, r, scale);
        }
    }

    // HUD hearts only under the hotbar HUD, never behind menus/shell screens.
    if !ui.open && ui.kind == GuiKind::Hotbar {
        if let Some(health) = ui.health {
            push_hearts(&mut build.hearts, screen, health, ui.heart_wiggle, scale);
        }
        push_effects(&mut build.effects, screen, &ui.effects, scale);
    }

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
}

fn push_recipe_hook_content(
    ui: &UiSnapshot,
    build: &mut UiBuild,
    hooks: &[crate::gui::DocHook],
    screen: (u32, u32),
    scale: f32,
) {
    for hook in hooks {
        let Some(recipe) = ui.craft_recipes.get(hook.index) else {
            continue;
        };
        match hook.kind {
            crate::gui::DocHookKind::CraftRecipeResult => {
                let side = hook.rect.w.min(hook.rect.h);
                let Some(clip) = effective_hook_clip(*hook) else {
                    continue;
                };
                build.hook_icon_quads.push(HookIconQuad {
                    item: recipe.result,
                    rect: SlotRect {
                        x: hook.rect.x + (hook.rect.w - side) * 0.5,
                        y: hook.rect.y + (hook.rect.h - side) * 0.5,
                        w: side,
                        h: side,
                    },
                    clip: Some(clip),
                    dim: !recipe.craftable,
                });
            }
            crate::gui::DocHookKind::CraftRecipeIngredients => {
                push_ingredient_strip(recipe, build, *hook, screen, scale);
            }
        }
    }
}

fn push_ingredient_strip(
    recipe: &crate::gui::CraftingRecipeView,
    build: &mut UiBuild,
    hook: crate::gui::DocHook,
    screen: (u32, u32),
    scale: f32,
) {
    if recipe.ingredients.is_empty() {
        return;
    }
    let Some(clip) = effective_hook_clip(hook) else {
        return;
    };
    let clip = Some(clip);
    let layout = ingredient_strip_layout(&recipe.ingredients, hook.rect.w, hook.rect.h, scale);
    let icon_side = layout.icon_side;
    let gap = 3.0 * scale;
    let mut x = hook.rect.x;
    for (index, (ingredient, count)) in recipe.ingredients.iter().take(layout.visible).enumerate() {
        let icon_rect = SlotRect {
            x,
            y: hook.rect.y + (hook.rect.h - icon_side) * 0.5,
            w: icon_side,
            h: icon_side,
        };
        build.hook_icon_quads.push(HookIconQuad {
            item: *ingredient,
            rect: icon_rect,
            clip,
            dim: !recipe.craftable,
        });
        x += icon_side + scale;
        let y = hook.rect.y + (hook.rect.h - tiny_text::GLYPH_H as f32 * scale) * 0.5;
        push_ingredient_count(
            &mut build.counts,
            screen,
            *count as u32,
            x,
            y,
            scale,
            clip,
            !recipe.craftable,
        );
        x += prefixed_number_width(*count as u32, scale);
        if index + 1 < layout.visible {
            x += gap;
        }
    }
    if layout.omitted > 0 {
        if layout.visible > 0 {
            x += gap;
        }
        let y = hook.rect.y + (hook.rect.h - tiny_text::GLYPH_H as f32 * scale) * 0.5;
        push_prefixed_number(
            &mut build.counts,
            screen,
            layout.omitted.min(u32::MAX as usize) as u32,
            x,
            y,
            scale,
            clip,
            !recipe.craftable,
            [0b010, 0b010, 0b111, 0b010, 0b010],
        );
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct IngredientStripLayout {
    visible: usize,
    omitted: usize,
    icon_side: f32,
}

fn ingredient_strip_layout(
    ingredients: &[(ItemType, u16)],
    width: f32,
    height: f32,
    scale: f32,
) -> IngredientStripLayout {
    let icon_side = height.min(12.0 * scale).max(0.0);
    let gap = 3.0 * scale;
    let pair_width =
        |count: u16| icon_side + scale + prefixed_number_width(u32::from(count), scale);
    let mut visible = ingredients.len();
    let mut pairs_width: f32 = ingredients
        .iter()
        .map(|(_, count)| pair_width(*count))
        .sum();
    loop {
        let omitted = ingredients.len() - visible;
        let pair_gaps = visible.saturating_sub(1) as f32 * gap;
        let overflow = if omitted == 0 {
            0.0
        } else {
            (if visible > 0 { gap } else { 0.0 })
                + prefixed_number_width(omitted.min(u32::MAX as usize) as u32, scale)
        };
        if pairs_width + pair_gaps + overflow <= width {
            return IngredientStripLayout {
                visible,
                omitted,
                icon_side,
            };
        }
        if visible == 0 {
            break;
        }
        visible -= 1;
        pairs_width -= pair_width(ingredients[visible].1);
    }
    IngredientStripLayout {
        visible: 0,
        omitted: ingredients.len(),
        icon_side,
    }
}

fn prefixed_number_width(number: u32, scale: f32) -> f32 {
    (tiny_text::GLYPH_W + 1 + tiny_text::number_width(number)) as f32 * scale
}

fn effective_hook_clip(hook: crate::gui::DocHook) -> Option<SlotRect> {
    hook.clip.map_or(Some(hook.rect), |inherited| {
        intersect_rect(hook.rect, inherited)
    })
}

fn push_ingredient_count(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    count: u32,
    x: f32,
    y: f32,
    scale: f32,
    clip: Option<SlotRect>,
    dim: bool,
) {
    push_prefixed_number(
        out,
        screen,
        count,
        x,
        y,
        scale,
        clip,
        dim,
        [0b000, 0b101, 0b010, 0b101, 0b000],
    );
}

#[allow(clippy::too_many_arguments)]
fn push_prefixed_number(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    count: u32,
    x: f32,
    y: f32,
    scale: f32,
    clip: Option<SlotRect>,
    dim: bool,
    prefix: [u8; tiny_text::GLYPH_H as usize],
) {
    let color = if dim {
        [0.62, 0.62, 0.62, 1.0]
    } else {
        [1.0, 1.0, 1.0, 1.0]
    };
    for (row, bits) in prefix.into_iter().enumerate() {
        for col in 0..tiny_text::GLYPH_W {
            if (bits >> (tiny_text::GLYPH_W - 1 - col)) & 1 == 1 {
                push_clipped_solid(
                    out,
                    screen,
                    SlotRect {
                        x: x + col as f32 * scale,
                        y: y + row as f32 * scale,
                        w: scale,
                        h: scale,
                    },
                    clip,
                    color,
                );
            }
        }
    }
    let digits_x = x + (tiny_text::GLYPH_W + 1) as f32 * scale;
    tiny_text::for_each_lit_cell(count, |px, py| {
        push_clipped_solid(
            out,
            screen,
            SlotRect {
                x: digits_x + px as f32 * scale,
                y: y + py as f32 * scale,
                w: scale,
                h: scale,
            },
            clip,
            color,
        );
    });
}

fn push_clipped_solid(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    rect: SlotRect,
    clip: Option<SlotRect>,
    color: [f32; 4],
) {
    let Some(rect) = clip.map_or(Some(rect), |clip| intersect_rect(rect, clip)) else {
        return;
    };
    push_solid(out, screen, rect.x, rect.y, rect.w, rect.h, color);
}

pub(super) fn intersect_rect(a: SlotRect, b: SlotRect) -> Option<SlotRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    (x1 > x0 && y1 > y0).then_some(SlotRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// The game state filling slot `i` of `role`, as `(item, count)` — the one place
/// that maps a slot role to its backing model. `None` for an empty slot or a
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
        Role::CraftResult => ui.craft_output.map(|(it, c)| (it, c as u32)),
        Role::FurnaceInput => ui.furnace.and_then(|f| f.input).map(stack),
        Role::FurnaceFuel => ui.furnace.and_then(|f| f.fuel).map(stack),
        Role::FurnaceOutput => ui.furnace.and_then(|f| f.output).map(stack),
        Role::Container => ui
            .container
            .as_ref()
            .and_then(|c| c.slots.get(i).copied().flatten())
            .map(stack),
        Role::Generic | Role::Other => None,
    }
}

/// Emit the bottom-left heart bar for `health` (half-heart points). Every heart gets an
/// empty container, then a full or half heart laid over it per the current health, so a
/// damaged heart shows the container through its missing portion — the vanilla read.
/// Scaled with the rest of the HUD. Called only for the [`GuiKind::Hotbar`] HUD.
///
/// `wiggle` is the app's active heart-wiggle burst (`(lo, hi, t)` — see
/// [`crate::gui::UiSnapshot::heart_wiggle`]): the hearts whose half-heart
/// points a heal just added or a hit just removed jitter fast for its
/// wall-clock window, so the change catches the eye at the exact hearts it
/// happened to.
fn push_hearts(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    health: crate::gui::HealthView,
    wiggle: Option<(i32, i32, f32)>,
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
    // Two incommensurate high frequencies (~29/24 Hz) so the burst reads as a
    // fast jitter rather than a bounce; amplitude ~1 logical px.
    let wiggle_offset = |i: i32| -> (f32, f32) {
        let Some((lo, hi, t)) = wiggle else {
            return (0.0, 0.0);
        };
        // Heart `i` covers half-heart points [2i, 2i + 2).
        if 2 * i >= hi || 2 * i + 2 <= lo {
            return (0.0, 0.0);
        }
        ((t * 183.0).sin() * 0.7 * scale, (t * 149.0).cos() * scale)
    };
    // Atlas cell `c` (0 empty, 1 half, 2 full) as top-left / bottom-right uv corners.
    let cell_uv = |c: i32| -> ([f32; 2], [f32; 2]) {
        let u0 = c as f32 * HEART_CELL_U;
        ([u0, 0.0], [u0 + HEART_CELL_U, 1.0])
    };
    let current = health.current.clamp(0, health.max);
    for i in 0..hearts {
        let (dx, dy) = wiggle_offset(i);
        let x = margin + i as f32 * step + dx;
        let y = y + dy;
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

/// Emit the status-effect icon row directly above the hearts: one cell per
/// active effect (in application order), each sampling its id-keyed cell of
/// the composed effect strip (frame + icon pre-composited — see
/// `render::effect_icons`). Called only for the [`GuiKind::Hotbar`] HUD.
fn push_effects(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    effects: &[crate::effect::Effect],
    scale: f32,
) {
    if effects.is_empty() {
        return;
    }
    let cell = crate::render::effect_icons::CELL_PX as f32 * scale;
    let gap = 2.0 * scale;
    let margin = HEART_MARGIN * scale;
    // Sits `gap` above the heart bar's top edge.
    let y = screen.1 as f32 - margin - HEART_PX * scale - gap - cell;
    let strip_cells = crate::effect::defs().len() as f32;
    for (i, effect) in effects.iter().enumerate() {
        let x = margin + i as f32 * (cell + gap);
        let u0 = effect.0 as f32 / strip_cells;
        let u1 = (effect.0 as f32 + 1.0) / strip_cells;
        push_quad_uv(out, screen, x, y, cell, cell, [u0, 0.0], [u1, 1.0], WHITE);
    }
}

/// Build the game-owned UI content for `ui` this frame. The buffers are the
/// caller-owned reusable [`UiBuild`] (cleared, capacity kept). `screen`,
/// `scale`, and `doc_slots` belong to the same stamped frame transaction.
pub fn build_ui(
    ui: &UiSnapshot,
    screen: (u32, u32),
    scale: f32,
    doc_slots: Option<&[crate::gui::DocSlot]>,
    doc_hooks: Option<&[crate::gui::DocHook]>,
    build: &mut UiBuild,
) {
    build.clear();

    if screen.0 == 0 || screen.1 == 0 {
        return;
    }
    if ui.hurt_flash > 0.0 {
        push_hurt_vignette(&mut build.vignette, screen, ui.hurt_flash);
    }
    if let Some(doc_slots) = doc_slots {
        push_doc_game_content(ui, build, doc_slots, screen, scale);
    }
    if let Some(doc_hooks) = doc_hooks {
        push_recipe_hook_content(ui, build, doc_hooks, screen, scale);
    }
}

/// Peak vignette alpha at full strength (reached toward the screen corners —
/// the shader's radial falloff scales it down toward the centre).
const VIGNETTE_MAX_ALPHA: f32 = 0.55;
const VIGNETTE_RED: [f32; 3] = [0.75, 0.03, 0.03];

/// uv sentinel marking the radial hurt vignette (see `ui.wgsl`): the fragment
/// stage computes a smooth falloff from the screen centre, so the overlay is
/// one connected ring rather than assembled bands.
const VIGNETTE_UV: [f32; 2] = [-2.0, -2.0];

/// The hurt flash: one fullscreen quad carrying the vignette sentinel; the
/// shader shapes it into a red radial rim (transparent centre, strongest at
/// the corners), scaled by `strength`.
fn push_hurt_vignette(out: &mut Vec<UiVertex>, screen: (u32, u32), strength: f32) {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    let a = VIGNETTE_MAX_ALPHA * strength.clamp(0.0, 1.0);
    push_quad_uv(
        out,
        screen,
        0.0,
        0.0,
        w,
        h,
        VIGNETTE_UV,
        VIGNETTE_UV,
        [VIGNETTE_RED[0], VIGNETTE_RED[1], VIGNETTE_RED[2], a],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::DocSlot;

    const SCREEN: (u32, u32) = (1280, 720);

    fn cell(role: Role, index: u32) -> DocSlot {
        DocSlot::new(
            role,
            index,
            SlotRect {
                x: 100.0 + index as f32 * 40.0,
                y: 100.0,
                w: 36.0,
                h: 36.0,
            },
        )
    }

    fn snap(kind: GuiKind, open: bool) -> UiSnapshot {
        let mut s = UiSnapshot {
            kind,
            open,
            cursor_px: (640.0, 360.0),
            active: 2,
            ..Default::default()
        };
        s.slots[0] = Some((ItemType::Stone, 64));
        s
    }

    fn build(ui: &UiSnapshot, slots: Option<&[DocSlot]>, out: &mut UiBuild) {
        build_ui(ui, SCREEN, crate::gui::gui_scale(SCREEN), slots, None, out);
    }

    #[test]
    fn gui_scale_is_clamped_and_increases_with_height() {
        assert_eq!(crate::gui::gui_scale((320, 240)), 1.0);
        assert!(crate::gui::gui_scale((1920, 1080)) >= 2.0);
        assert_eq!(crate::gui::gui_scale((10, 10)), 1.0);
        assert_eq!(crate::gui::gui_scale((10000, 10000)), 4.0);
    }

    #[test]
    fn zero_screen_and_missing_document_build_nothing() {
        let mut b = UiBuild::default();
        let s = snap(GuiKind::Hotbar, false);
        let slots = [cell(Role::Hotbar, 0)];
        build_ui(&s, (0, 0), 1.0, Some(&slots), None, &mut b);
        assert!(b.icon_quads.is_empty() && b.hearts.is_empty());

        // No solved document (and therefore no slots): game content draws nothing.
        let mut s = snap(GuiKind::Inventory, true);
        s.cursor = Some((ItemType::Dirt, 12));
        build(&s, None, &mut b);
        assert!(b.icon_quads.is_empty() && b.drag_icon_quads.is_empty() && b.counts.is_empty());
    }

    #[test]
    fn recipe_hooks_emit_result_ingredients_counts_and_scroll_clips() {
        let mut snapshot = snap(GuiKind::Inventory, true);
        snapshot.craft_recipes.push(crate::gui::CraftingRecipeView {
            result: ItemType::Stick,
            ingredients: vec![(ItemType::Coal, 2), (ItemType::Dirt, 3)],
            craftable: false,
        });
        let clip = SlotRect {
            x: 100.0,
            y: 105.0,
            w: 160.0,
            h: 20.0,
        };
        let hooks = [
            crate::gui::DocHook {
                kind: crate::gui::DocHookKind::CraftRecipeResult,
                index: 0,
                rect: SlotRect {
                    x: 100.0,
                    y: 100.0,
                    w: 20.0,
                    h: 20.0,
                },
                clip: Some(clip),
            },
            crate::gui::DocHook {
                kind: crate::gui::DocHookKind::CraftRecipeIngredients,
                index: 0,
                rect: SlotRect {
                    x: 124.0,
                    y: 105.0,
                    w: 120.0,
                    h: 14.0,
                },
                clip: Some(clip),
            },
            crate::gui::DocHook {
                kind: crate::gui::DocHookKind::CraftRecipeResult,
                index: 0,
                rect: SlotRect {
                    x: 100.0,
                    y: 200.0,
                    w: 20.0,
                    h: 20.0,
                },
                clip: Some(clip),
            },
            crate::gui::DocHook {
                kind: crate::gui::DocHookKind::CraftRecipeIngredients,
                index: 0,
                rect: SlotRect {
                    x: 124.0,
                    y: 200.0,
                    w: 120.0,
                    h: 14.0,
                },
                clip: Some(clip),
            },
        ];
        let mut build = UiBuild::default();

        build_ui(
            &snapshot,
            SCREEN,
            crate::gui::gui_scale(SCREEN),
            None,
            Some(&hooks),
            &mut build,
        );

        assert_eq!(
            build.hook_icon_quads.len(),
            3,
            "fully clipped recipe rows emit no host content"
        );
        assert!(build.hook_icon_quads.iter().all(|icon| icon.dim));
        assert!(build.hook_icon_quads.iter().all(|icon| icon.clip.is_some()));
        assert!(
            !build.counts.is_empty(),
            "ingredient ×N labels are host-drawn"
        );
    }

    #[test]
    fn oversized_ingredient_strips_keep_icons_readable_and_report_omissions() {
        let ingredients = vec![(ItemType::Coal, u16::MAX); 12];
        let scale = 3.0;
        let compact = ingredient_strip_layout(&ingredients, 120.0 * scale, 12.0 * scale, scale);
        assert!(compact.visible > 0 && compact.visible < ingredients.len());
        assert_eq!(compact.visible + compact.omitted, ingredients.len());
        assert!(compact.icon_side >= 8.0 * scale);

        let roomy = ingredient_strip_layout(&ingredients, 10_000.0, 12.0 * scale, scale);
        assert_eq!(roomy.visible, ingredients.len());
        assert_eq!(roomy.omitted, 0);
    }

    #[test]
    fn doc_slots_emit_icons_counts_and_the_drag_stack() {
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Inventory, true);
        let slots = [cell(Role::Hotbar, 0), cell(Role::Hotbar, 1)];
        s.cursor = Some((ItemType::Dirt, 12));
        build(&s, Some(&slots), &mut b);
        assert_eq!(b.icon_quads.len(), 1, "only the filled cell draws an icon");
        let (item, r) = b.icon_quads[0];
        assert_eq!(item, ItemType::Stone);
        let outer = cell(Role::Hotbar, 0).rect;
        assert!(
            r.x > outer.x && r.y > outer.y && r.w < outer.w,
            "icon is inset inside the themed cell"
        );
        assert!(!b.counts.is_empty(), "stack count 64 drawn");
        assert_eq!(b.drag_icon_quads.len(), 1, "cursor-held stack drawn");
        assert!(!b.drag_counts.is_empty(), "drag count > 1 drawn");
    }

    #[test]
    fn hearts_only_on_the_hotbar_hud_with_health() {
        let health = Some(crate::gui::HealthView {
            current: 15,
            max: 20,
        });
        // HUD (kind Hotbar, no open menu) with health: hearts drawn.
        let mut b = UiBuild::default();
        let slots = [cell(Role::Hotbar, 0)];
        let mut s = snap(GuiKind::Hotbar, false);
        s.health = health;
        build(&s, Some(&slots), &mut b);
        assert!(!b.hearts.is_empty(), "the HUD draws the heart bar");

        // Behind an open menu (kind != Hotbar): hidden even with health.
        let mut s = snap(GuiKind::Inventory, true);
        s.health = health;
        build(&s, Some(&slots), &mut b);
        assert!(b.hearts.is_empty(), "no hearts behind an open menu");

        // On the HUD but with no health (a spectator): still nothing.
        build(&snap(GuiKind::Hotbar, false), Some(&slots), &mut b);
        assert!(b.hearts.is_empty(), "no hearts without survival health");
    }

    #[test]
    fn build_reuses_buffers_without_growth() {
        let mut b = UiBuild::default();
        let slots = vec![cell(Role::Hotbar, 0)];
        build(&snap(GuiKind::Inventory, true), Some(&slots), &mut b);
        let cap = b.icon_quads.capacity();
        assert!(cap > 0);
        build(&snap(GuiKind::Hotbar, false), Some(&slots), &mut b);
        assert_eq!(b.icon_quads.capacity(), cap, "icon-quad buffer reused");
    }
}
