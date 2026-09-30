pub mod icon;

use petramond::gui::{Role, SlotRect, UiSnapshot};

use petramond_text::tiny as tiny_text;
use petramond_world::gui_state::GuiKind;
use petramond_world::inventory::HOTBAR_LEN;
use petramond_world::item::ItemType;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

pub(super) const SOLID_UV: [f32; 2] = [-1.0, -1.0];

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

pub(super) const SLOT_PX: f32 = 16.0;

const HEART_PX: f32 = 9.0;
const HEART_STEP: f32 = 8.0;
const HEART_MARGIN: f32 = 8.0;
const HEART_CELL_U: f32 = 1.0 / 3.0;

#[derive(Default)]
pub struct UiBuild {
    pub hearts: Vec<UiVertex>,
    pub icon_quads: Vec<(ItemType, SlotRect, [f32; 4], bool)>,
    pub hook_icon_quads: Vec<HookIconQuad>,
    pub counts: Vec<UiVertex>,
    pub overlay_icon_quads: Vec<HookIconQuad>,
    pub overlay_counts: Vec<UiVertex>,
    pub drag_icon_quads: Vec<(ItemType, SlotRect, [f32; 4], bool)>,
    pub drag_counts: Vec<UiVertex>,
    pub vignette: Vec<UiVertex>,
    pub effects: Vec<UiVertex>,
}

impl UiBuild {
    fn clear(&mut self) {
        self.hearts.clear();
        self.effects.clear();
        self.icon_quads.clear();
        self.hook_icon_quads.clear();
        self.counts.clear();
        self.overlay_icon_quads.clear();
        self.overlay_counts.clear();
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

#[inline]
pub(super) fn pixel_to_ndc(screen: (u32, u32), x: f32, y: f32) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0]
}

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

#[allow(clippy::too_many_arguments)]
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
    out.push(v(p_tl, uv_tl));
    out.push(v(p_bl, uv_bl));
    out.push(v(p_br, uv_br));
    out.push(v(p_tl, uv_tl));
    out.push(v(p_br, uv_br));
    out.push(v(p_tr, uv_tr));
}

fn push_doc_game_content(
    ui: &UiSnapshot,
    build: &mut UiBuild,
    slots: &[petramond::gui::DocSlot],
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
        let Some(stack) = slot_item(ui, slot.role, i) else {
            continue;
        };
        if stack.item == ItemType::Air || stack.count == 0 {
            continue;
        }
        if slot.raised {
            let tier = build.tier(true);
            tier.icons.push(HookIconQuad {
                item: stack.item,
                rect: r,
                clip: None,
                dim: false,
            });
            for overlay in petramond_world::item::variant::overlay_items(stack.variant) {
                tier.icons.push(HookIconQuad {
                    item: overlay,
                    rect: r,
                    clip: None,
                    dim: false,
                });
            }
            if stack.count > 1 {
                icon::push_count(tier.counts, screen, stack.count as u32, r, scale);
            }
            continue;
        }
        icon::push_slot_icon(build, screen, &stack, r);
        if stack.count > 1 {
            icon::push_count(&mut build.counts, screen, stack.count as u32, r, scale);
        }
    }

    if !ui.open && ui.kind == GuiKind::Hotbar {
        if let Some(health) = ui.health {
            push_hearts(&mut build.hearts, screen, health, ui.heart_wiggle, scale);
        }
        push_effects(&mut build.effects, screen, &ui.effects, scale);
    }

    if ui.open {
        if let Some(stack) = ui.cursor {
            if stack.item != ItemType::Air && stack.count > 0 {
                let s = SLOT_PX * scale;
                let (cx, cy) = ui.cursor_px;
                let r = SlotRect {
                    x: cx - s * 0.5,
                    y: cy - s * 0.5,
                    w: s,
                    h: s,
                };
                icon::push_stack_quads(&mut build.drag_icon_quads, &stack, r);
                if stack.count > 1 {
                    icon::push_count(&mut build.drag_counts, screen, stack.count as u32, r, scale);
                }
            }
        }
    }
}

struct HookTier<'a> {
    icons: &'a mut Vec<HookIconQuad>,
    counts: &'a mut Vec<UiVertex>,
}

impl UiBuild {
    fn tier(&mut self, overlay: bool) -> HookTier<'_> {
        if overlay {
            HookTier {
                icons: &mut self.overlay_icon_quads,
                counts: &mut self.overlay_counts,
            }
        } else {
            HookTier {
                icons: &mut self.hook_icon_quads,
                counts: &mut self.counts,
            }
        }
    }
}

fn push_recipe_hook_content(
    ui: &UiSnapshot,
    build: &mut UiBuild,
    hooks: &[petramond::gui::DocHook],
    screen: (u32, u32),
    scale: f32,
) {
    use petramond::gui::DocHookKind as Kind;
    for hook in hooks {
        if let Kind::ItemView { item, dim } = hook.kind {
            let side = hook.rect.w.min(hook.rect.h);
            let Some(clip) = effective_hook_clip(*hook) else {
                continue;
            };
            build.tier(hook.overlay).icons.push(HookIconQuad {
                item,
                rect: SlotRect {
                    x: hook.rect.x + (hook.rect.w - side) * 0.5,
                    y: hook.rect.y + (hook.rect.h - side) * 0.5,
                    w: side,
                    h: side,
                },
                clip: Some(clip),
                dim,
            });
            continue;
        }
        let recipe = match hook.kind {
            Kind::RecipeResult => ui.craft_recipes.get(hook.index),
            Kind::TipResult | Kind::TipIngredients => ui.craft_tip.as_ref(),
            Kind::ItemView { .. } => unreachable!("handled above"),
        };
        let Some(recipe) = recipe else {
            continue;
        };
        match hook.kind {
            Kind::RecipeResult | Kind::TipResult => {
                let side = hook.rect.w.min(hook.rect.h);
                let Some(clip) = effective_hook_clip(*hook) else {
                    continue;
                };
                build.tier(hook.overlay).icons.push(HookIconQuad {
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
            Kind::TipIngredients => {
                push_ingredient_strip(recipe, build, *hook, screen, scale);
            }
            Kind::ItemView { .. } => unreachable!("handled above"),
        }
    }
}

fn push_ingredient_strip(
    recipe: &petramond::gui::CraftingRecipeView,
    build: &mut UiBuild,
    hook: petramond::gui::DocHook,
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
    let tier = build.tier(hook.overlay);
    for (index, (ingredient, count)) in recipe.ingredients.iter().take(layout.visible).enumerate() {
        let icon_rect = SlotRect {
            x,
            y: hook.rect.y + (hook.rect.h - icon_side) * 0.5,
            w: icon_side,
            h: icon_side,
        };
        tier.icons.push(HookIconQuad {
            item: *ingredient,
            rect: icon_rect,
            clip,
            dim: !recipe.craftable,
        });
        x += icon_side + scale;
        let y = hook.rect.y + (hook.rect.h - tiny_text::GLYPH_H as f32 * scale) * 0.5;
        push_ingredient_count(
            tier.counts,
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
            tier.counts,
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

pub fn ingredient_strip_width(ingredients: &[(ItemType, u16)]) -> i32 {
    if ingredients.is_empty() {
        return 0;
    }
    let pairs: i32 = ingredients
        .iter()
        .map(|(_, count)| STRIP_ICON_SIDE + 1 + prefixed_number_width_logical(u32::from(*count)))
        .sum();
    pairs + STRIP_GAP * (ingredients.len() as i32 - 1)
}

const STRIP_ICON_SIDE: i32 = 12;
const STRIP_GAP: i32 = 3;

fn prefixed_number_width_logical(number: u32) -> i32 {
    (tiny_text::GLYPH_W + 1 + tiny_text::number_width(number)) as i32
}

fn ingredient_strip_layout(
    ingredients: &[(ItemType, u16)],
    width: f32,
    height: f32,
    scale: f32,
) -> IngredientStripLayout {
    let icon_side = height.min(STRIP_ICON_SIDE as f32 * scale).max(0.0);
    let gap = STRIP_GAP as f32 * scale;
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
    prefixed_number_width_logical(number) as f32 * scale
}

fn effective_hook_clip(hook: petramond::gui::DocHook) -> Option<SlotRect> {
    hook.clip.map_or(Some(hook.rect), |inherited| {
        intersect_rect(hook.rect, inherited)
    })
}

#[allow(clippy::too_many_arguments)]
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

fn slot_item(ui: &UiSnapshot, role: Role, i: usize) -> Option<petramond_world::item::ItemStack> {
    match role {
        Role::Hotbar => ui.slots.get(i).copied().flatten(),
        Role::PlayerInv => ui.slots.get(HOTBAR_LEN + i).copied().flatten(),
        Role::OffHand => ui.off_hand,
        Role::CraftResult => ui.craft_output,
        Role::Container => ui
            .container
            .as_ref()
            .and_then(|c| c.slots.get(i).copied().flatten()),
        Role::Generic | Role::Other => None,
    }
}

fn push_hearts(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    health: petramond_world::gui_state::HealthView,
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
    let wiggle_offset = |i: i32| -> (f32, f32) {
        let Some((lo, hi, t)) = wiggle else {
            return (0.0, 0.0);
        };
        if 2 * i >= hi || 2 * i + 2 <= lo {
            return (0.0, 0.0);
        }
        ((t * 183.0).sin() * 0.7 * scale, (t * 149.0).cos() * scale)
    };
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
        let cell = match (current - i * 2).clamp(0, 2) {
            2 => 2,
            1 => 1,
            _ => continue,
        };
        let (tl, br) = cell_uv(cell);
        push_quad_uv(out, screen, x, y, size, size, tl, br, WHITE);
    }
}

fn push_effects(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    effects: &[petramond_world::effect::Effect],
    scale: f32,
) {
    if effects.is_empty() {
        return;
    }
    let cell = crate::effect_icons::CELL_PX as f32 * scale;
    let gap = 2.0 * scale;
    let margin = HEART_MARGIN * scale;
    let y = screen.1 as f32 - margin - HEART_PX * scale - gap - cell;
    let strip_cells = petramond_world::effect::defs().len() as f32;
    for (i, effect) in effects.iter().enumerate() {
        let x = margin + i as f32 * (cell + gap);
        let u0 = effect.0 as f32 / strip_cells;
        let u1 = (effect.0 as f32 + 1.0) / strip_cells;
        push_quad_uv(out, screen, x, y, cell, cell, [u0, 0.0], [u1, 1.0], WHITE);
    }
}

pub fn build_ui(
    ui: &UiSnapshot,
    screen: (u32, u32),
    scale: f32,
    doc_slots: Option<&[petramond::gui::DocSlot]>,
    doc_hooks: Option<&[petramond::gui::DocHook]>,
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

const VIGNETTE_MAX_ALPHA: f32 = 0.55;
const VIGNETTE_RED: [f32; 3] = [0.75, 0.03, 0.03];

const VIGNETTE_UV: [f32; 2] = [-2.0, -2.0];

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
    use petramond::gui::DocSlot;

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
        s.slots[0] = Some(petramond_world::item::ItemStack::new(ItemType::Stone, 64));
        s
    }

    fn build(ui: &UiSnapshot, slots: Option<&[DocSlot]>, out: &mut UiBuild) {
        build_ui(
            ui,
            SCREEN,
            petramond::gui::gui_scale(SCREEN),
            slots,
            None,
            out,
        );
    }

    #[test]
    fn zero_screen_and_missing_document_build_nothing() {
        let mut b = UiBuild::default();
        let s = snap(GuiKind::Hotbar, false);
        let slots = [cell(Role::Hotbar, 0)];
        build_ui(&s, (0, 0), 1.0, Some(&slots), None, &mut b);
        assert!(b.icon_quads.is_empty() && b.hearts.is_empty());

        let mut s = snap(GuiKind::Inventory, true);
        s.cursor = Some(petramond_world::item::ItemStack::new(ItemType::Dirt, 12));
        build(&s, None, &mut b);
        assert!(b.icon_quads.is_empty() && b.drag_icon_quads.is_empty() && b.counts.is_empty());
    }

    fn recipe(result: ItemType, craftable: bool) -> petramond::gui::CraftingRecipeView {
        petramond::gui::CraftingRecipeView {
            result,
            ingredients: vec![(ItemType::Coal, 2), (ItemType::Dirt, 3)],
            craftable,
        }
    }

    fn hook(
        kind: petramond::gui::DocHookKind,
        rect: SlotRect,
        clip: Option<SlotRect>,
        overlay: bool,
    ) -> petramond::gui::DocHook {
        petramond::gui::DocHook {
            kind,
            index: 0,
            rect,
            clip,
            overlay,
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> SlotRect {
        SlotRect { x, y, w, h }
    }

    #[test]
    fn recipe_grid_cells_dim_unaffordable_results_and_respect_scroll_clips() {
        let mut snapshot = snap(GuiKind::Inventory, true);
        snapshot.craft_recipes.push(recipe(ItemType::Stick, false));
        let clip = rect(100.0, 105.0, 160.0, 20.0);
        let hooks = [
            hook(
                petramond::gui::DocHookKind::RecipeResult,
                rect(100.0, 100.0, 20.0, 20.0),
                Some(clip),
                false,
            ),
            hook(
                petramond::gui::DocHookKind::RecipeResult,
                rect(100.0, 200.0, 20.0, 20.0),
                Some(clip),
                false,
            ),
        ];
        let mut build = UiBuild::default();

        build_ui(
            &snapshot,
            SCREEN,
            petramond::gui::gui_scale(SCREEN),
            None,
            Some(&hooks),
            &mut build,
        );

        assert_eq!(
            build.hook_icon_quads.len(),
            1,
            "a fully clipped cell emits no host content"
        );
        assert!(build.hook_icon_quads[0].dim, "unaffordable results dim");
        assert!(build.hook_icon_quads[0].clip.is_some());
    }

    #[test]
    fn tooltip_hook_content_goes_to_the_overlay_tier_and_grid_content_does_not() {
        let mut snapshot = snap(GuiKind::Inventory, true);
        snapshot.craft_recipes.push(recipe(ItemType::Stick, true));
        snapshot.craft_tip = Some(recipe(ItemType::Stone, true));
        let hooks = [
            hook(
                petramond::gui::DocHookKind::RecipeResult,
                rect(10.0, 10.0, 18.0, 18.0),
                None,
                false,
            ),
            hook(
                petramond::gui::DocHookKind::TipResult,
                rect(200.0, 200.0, 18.0, 18.0),
                None,
                true,
            ),
            hook(
                petramond::gui::DocHookKind::TipIngredients,
                rect(220.0, 200.0, 120.0, 14.0),
                None,
                true,
            ),
        ];
        let mut build = UiBuild::default();

        build_ui(
            &snapshot,
            SCREEN,
            petramond::gui::gui_scale(SCREEN),
            None,
            Some(&hooks),
            &mut build,
        );

        let base: Vec<ItemType> = build.hook_icon_quads.iter().map(|q| q.item).collect();
        assert!(base.contains(&ItemType::Stick), "{base:?}");
        assert!(!base.contains(&ItemType::Stone), "tooltip leaked: {base:?}");

        let over: Vec<ItemType> = build.overlay_icon_quads.iter().map(|q| q.item).collect();
        assert!(over.contains(&ItemType::Stone), "{over:?}");
        assert!(!over.contains(&ItemType::Stick), "{over:?}");
        assert!(
            !build.overlay_counts.is_empty(),
            "tooltip ×N labels draw over the tooltip panel, not under it"
        );
    }

    #[test]
    fn tooltip_hooks_draw_nothing_without_a_named_recipe() {
        let mut snapshot = snap(GuiKind::Inventory, true);
        snapshot.craft_recipes.push(recipe(ItemType::Stick, true));
        let hooks = [hook(
            petramond::gui::DocHookKind::TipIngredients,
            rect(220.0, 200.0, 120.0, 14.0),
            None,
            true,
        )];
        let mut build = UiBuild::default();
        build_ui(
            &snapshot,
            SCREEN,
            petramond::gui::gui_scale(SCREEN),
            None,
            Some(&hooks),
            &mut build,
        );
        assert!(build.overlay_icon_quads.is_empty());
        assert!(build.overlay_counts.is_empty());
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
    fn strip_fits_the_width_it_asks_for() {
        for count in [1u16, 9, 10, 64, u16::MAX] {
            for n in 1..=12usize {
                let ingredients = vec![(ItemType::Coal, count); n];
                let asked = ingredient_strip_width(&ingredients);
                for scale in [1.0f32, 2.0, 3.0] {
                    let fit = ingredient_strip_layout(
                        &ingredients,
                        asked as f32 * scale,
                        12.0 * scale,
                        scale,
                    );
                    assert_eq!(
                        fit.omitted, 0,
                        "×{count} × {n} at scale {scale}: asked {asked} logical px and still \
                         dropped {} ingredient(s)",
                        fit.omitted
                    );
                }
            }
        }
        assert_eq!(ingredient_strip_width(&[]), 0, "no strip, no room");
    }

    #[test]
    fn doc_slots_emit_icons_counts_and_the_drag_stack() {
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Inventory, true);
        let slots = [cell(Role::Hotbar, 0), cell(Role::Hotbar, 1)];
        s.cursor = Some(petramond_world::item::ItemStack::new(ItemType::Dirt, 12));
        build(&s, Some(&slots), &mut b);
        assert_eq!(b.icon_quads.len(), 1, "only the filled cell draws an icon");
        let (item, r, _color, _dyed) = b.icon_quads[0];
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
        let health = Some(petramond_world::gui_state::HealthView {
            current: 15,
            max: 20,
        });
        let mut b = UiBuild::default();
        let slots = [cell(Role::Hotbar, 0)];
        let mut s = snap(GuiKind::Hotbar, false);
        s.health = health;
        build(&s, Some(&slots), &mut b);
        assert!(!b.hearts.is_empty(), "the HUD draws the heart bar");

        let mut s = snap(GuiKind::Inventory, true);
        s.health = health;
        build(&s, Some(&slots), &mut b);
        assert!(b.hearts.is_empty(), "no hearts behind an open menu");

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
