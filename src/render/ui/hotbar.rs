//! The closed-HUD hotbar widget: the `hotbar.png` strip centred at the screen
//! bottom plus the selection highlight over the active slot. Owns the hotbar's own
//! pixel layout (a 20px slot pitch, distinct from the open panel's 18px) and the
//! [`hotbar_top_ndc`] threshold the first-person hand clearance test pins to.

use super::super::renderer::UiSnapshot;
use super::super::resources::GuiSprite;
use super::{push_sprite, SlotRect, UiBuild, HOTBAR_LEN, SLOT_PX};

/// Closed-hotbar slot pitch (px): the `hotbar.png` widget is 182px = 20×9 + 2, so
/// its 9 slots sit at a 20px pitch (16px interior + 2px border each side).
const HOTBAR_PITCH: f32 = 20.0;
/// Left inset (px) from the hotbar sprite's left edge to the first slot interior.
const HOTBAR_SLOT_INSET: f32 = 3.0;
/// Bottom margin (px) from the screen bottom edge to the hotbar sprite.
const HOTBAR_BOTTOM_MARGIN: f32 = 1.0;

/// The hotbar sprite's top-left pixel position for `screen` at `scale`.
pub(super) fn hotbar_origin(screen: (u32, u32), scale: f32) -> (f32, f32) {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    let (sw, sh) = GuiSprite::Hotbar.size_px();
    let bw = sw as f32 * scale;
    let bh = sh as f32 * scale;
    let x = (w - bw) * 0.5;
    let y = h - bh - HOTBAR_BOTTOM_MARGIN * scale;
    (x, y)
}

/// The interior pixel rect of closed-hotbar slot `i` (`0..HOTBAR_LEN`), or `None`
/// out of range. The single source of truth for both drawing the closed HUD icons
/// and hit-testing them; the open panel uses `inventory::panel_slot_rect` instead.
pub(super) fn hotbar_slot_rect(i: usize, screen: (u32, u32), scale: f32) -> Option<SlotRect> {
    if i >= HOTBAR_LEN {
        return None;
    }
    let s = scale;
    let (ox, oy) = hotbar_origin(screen, s);
    let x = ox + (HOTBAR_SLOT_INSET + i as f32 * HOTBAR_PITCH) * s;
    // The 22px-tall hotbar sprite has a 3px top border, so the 16px slot
    // interior starts HOTBAR_SLOT_INSET px below the bar top — centring the
    // icon vertically in the slot row (interior 3..19, bottom border 19..22).
    let y = oy + HOTBAR_SLOT_INSET * s;
    Some(SlotRect {
        x,
        y,
        w: SLOT_PX * s,
        h: SLOT_PX * s,
    })
}

/// Draw the closed HUD background: the centred hotbar strip plus the selection
/// highlight over the active slot. Called by [`super::build_ui`] when the inventory
/// is closed, before the per-slot icons.
pub(super) fn build_background(ui: &UiSnapshot, build: &mut UiBuild, screen: (u32, u32), scale: f32) {
    // The hotbar strip, centred at the bottom.
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
    if let Some(r) = hotbar_slot_rect(active, screen, scale) {
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
    let scale = super::gui_scale(screen);
    let (_ox, oy) = hotbar_origin(screen, scale);
    // `oy` is the hotbar sprite's TOP in physical pixels (top-left origin, y down).
    1.0 - oy / screen.1 as f32 * 2.0
}

#[cfg(test)]
mod tests {
    use super::super::inventory::slot_rect;
    use super::super::tests::{empty_build, snap_open};
    use super::super::gui_scale;
    use super::*;
    use crate::inventory::TOTAL_SLOTS;

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
    fn closed_build_has_hotbar_sprite_and_icons() {
        let mut build = empty_build();
        super::super::build_ui(&snap_open(false), &mut build);
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
}
