//! The furnace screen: the input / fuel / output slots plus the lit smelt-arrow
//! and fuel-flame gauges painted over the panel's empty outlines. Owns the furnace
//! slot layout (the single source of truth shared by the renderer and the App's
//! [`furnace_slot_at_cursor`] hit-test) and the gauge geometry.

use super::super::renderer::UiSnapshot;
use super::super::resources::GuiSprite;
use super::{gui_scale, icon, panel_origin, push_quad_uv, SlotRect, UiBuild, UiVertex, SLOT_PX};
use crate::item::ItemType;

/// Furnace screen interior layout (px), relative to the 176×166 panel's top-left,
/// matching the vanilla `furnace.png` art: input slot above the flame, fuel below
/// it, output to the right, with the smelt arrow and the fuel flame between.
const FURNACE_INPUT: (f32, f32) = (56.0, 17.0);
const FURNACE_FUEL: (f32, f32) = (56.0, 53.0);
const FURNACE_OUTPUT: (f32, f32) = (116.0, 35.0);
/// Top-left of the lit smelt arrow (24×16), which fills left→right with cook
/// progress over the panel's empty arrow. The sprite's arrow bar sits at sprite
/// row 7, and the panel's empty arrow bar at panel y=42, so the sprite is drawn at
/// y=35 to line the two up.
const FURNACE_ARROW: (f32, f32) = (79.0, 35.0);
/// Top-left of the lit fuel flame (14×14), which depletes top→down with remaining
/// burn time over the panel's empty flame.
const FURNACE_FLAME: (f32, f32) = (56.0, 36.0);

/// A hit-tested furnace slot: the smeltable input, the fuel, or the output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FurnaceHit {
    Input,
    Fuel,
    Output,
}

/// Interior pixel rect of furnace slot `slot`, panel-relative like the inventory.
fn furnace_slot_rect(slot: FurnaceHit, screen: (u32, u32), scale: f32) -> SlotRect {
    let (ox, oy) = panel_origin(screen, scale);
    let (sx, sy) = match slot {
        FurnaceHit::Input => FURNACE_INPUT,
        FurnaceHit::Fuel => FURNACE_FUEL,
        FurnaceHit::Output => FURNACE_OUTPUT,
    };
    SlotRect {
        x: ox + sx * scale,
        y: oy + sy * scale,
        w: SLOT_PX * scale,
        h: SLOT_PX * scale,
    }
}

/// The furnace slot under the cursor, or `None`. Shared with the App so a visible
/// furnace slot is always the one that gets clicked.
pub fn furnace_slot_at_cursor(screen: (u32, u32), cursor_px: (f32, f32)) -> Option<FurnaceHit> {
    let scale = gui_scale(screen);
    let (px, py) = cursor_px;
    [FurnaceHit::Input, FurnaceHit::Fuel, FurnaceHit::Output]
        .into_iter()
        .find(|&slot| furnace_slot_rect(slot, screen, scale).contains(px, py))
}

/// Push the lit furnace smelt-arrow cropped to `frac` of its width (left→right),
/// painted over the panel's empty arrow outline. No-op at `frac <= 0`.
fn push_furnace_arrow(out: &mut Vec<UiVertex>, screen: (u32, u32), scale: f32, frac: f32) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let (ox, oy) = panel_origin(screen, scale);
    let (ax, ay) = FURNACE_ARROW;
    let (fw, fh) = GuiSprite::FurnaceArrow.size_px();
    let [u0, v0, u1, v1] = GuiSprite::FurnaceArrow.rect();
    let u_mid = u0 + (u1 - u0) * frac;
    push_quad_uv(
        out,
        screen,
        ox + ax * scale,
        oy + ay * scale,
        fw as f32 * frac * scale,
        fh as f32 * scale,
        [u0, v0],
        [u_mid, v1],
        [1.0, 1.0, 1.0, 1.0],
    );
}

/// Push the lit furnace flame cropped to the bottom `frac` of its height — it
/// burns down from the top as fuel depletes — over the panel's empty flame. No-op
/// at `frac <= 0`.
fn push_furnace_flame(out: &mut Vec<UiVertex>, screen: (u32, u32), scale: f32, frac: f32) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let (ox, oy) = panel_origin(screen, scale);
    let (fx, fy) = FURNACE_FLAME;
    let (fw, fh) = GuiSprite::FurnaceFlame.size_px();
    let [u0, v0, u1, v1] = GuiSprite::FurnaceFlame.rect();
    let h = fh as f32 * frac;
    let top_off = fh as f32 - h; // hidden pixels at the top
    let v_top = v0 + (v1 - v0) * (1.0 - frac);
    push_quad_uv(
        out,
        screen,
        ox + fx * scale,
        oy + (fy + top_off) * scale,
        fw as f32 * scale,
        h * scale,
        [u0, v_top],
        [u1, v1],
        [1.0, 1.0, 1.0, 1.0],
    );
}

/// Draw the furnace slots' icons + the lit smelt/burn gauges (furnace screen only).
/// Called by [`super::build_ui`] when the open panel is a furnace.
pub(super) fn build(ui: &UiSnapshot, build: &mut UiBuild, screen: (u32, u32), scale: f32) {
    let Some(furnace) = ui.furnace else { return };
    for (slot, stack) in [
        (FurnaceHit::Input, furnace.input),
        (FurnaceHit::Fuel, furnace.fuel),
        (FurnaceHit::Output, furnace.output),
    ] {
        let Some(stack) = stack else { continue };
        if stack.item == ItemType::Air || stack.count == 0 {
            continue;
        }
        let r = furnace_slot_rect(slot, screen, scale);
        icon::push_slot_icon(build, screen, stack.item, r);
        if stack.count > 1 {
            icon::push_count(
                &mut build.overlay_verts,
                screen,
                stack.count as u32,
                r,
                scale,
            );
        }
    }
    // Lit gauges paint over the panel's empty arrow/flame outlines.
    push_furnace_arrow(&mut build.verts, screen, scale, furnace.cook01);
    push_furnace_flame(&mut build.verts, screen, scale, furnace.burn01);
}

#[cfg(test)]
mod tests {
    use super::super::inventory::{cursor_in_panel, slot_at_cursor};
    use super::super::tests::{empty_build, snap_open};
    use super::*;

    #[test]
    fn furnace_slots_hit_test_round_trips() {
        let screen = (1280u32, 720u32);
        let scale = gui_scale(screen);
        for slot in [FurnaceHit::Input, FurnaceHit::Fuel, FurnaceHit::Output] {
            let r = furnace_slot_rect(slot, screen, scale);
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert_eq!(furnace_slot_at_cursor(screen, c), Some(slot));
            // Furnace slots sit inside the panel and are not inventory slots.
            assert!(cursor_in_panel(screen, c), "{slot:?} inside panel");
            assert_eq!(
                slot_at_cursor(screen, true, c),
                None,
                "{slot:?} not an inv slot"
            );
        }
        // The three slots are distinct positions.
        let centre = |s| {
            let r = furnace_slot_rect(s, screen, scale);
            (r.x, r.y)
        };
        assert_ne!(centre(FurnaceHit::Input), centre(FurnaceHit::Fuel));
        assert_ne!(centre(FurnaceHit::Input), centre(FurnaceHit::Output));
    }

    #[test]
    fn furnace_screen_draws_its_slots_and_skips_the_craft_grid() {
        use crate::item::ItemStack;
        let mut build = empty_build();
        let mut s = snap_open(true);
        // A craft cell that would draw on a crafting screen but must be skipped here.
        s.craft[0] = Some((ItemType::OakPlanks, 1));
        s.furnace = Some(crate::render::FurnaceView {
            input: Some(ItemStack::new(ItemType::RawIron, 5)),
            fuel: Some(ItemStack::new(ItemType::Coal, 3)),
            output: Some(ItemStack::new(ItemType::IronIngot, 1)),
            cook01: 0.5,
            burn01: 0.5,
        });
        super::super::build_ui(&s, &mut build);
        // 2 hotbar items (snap_open) + 3 furnace slots = 5 icon quads; the craft cell
        // is NOT drawn (the furnace panel replaces the grid).
        assert_eq!(build.icon_quads.len(), 5);
        // The lit arrow + flame add gui quads over the panel background.
        assert!(build.verts.len() > 12, "panel + gauges drawn");
    }
}
