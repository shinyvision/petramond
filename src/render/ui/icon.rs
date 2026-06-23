//! Per-slot item icon projection + stack-count digits: the model3d icon for an
//! item placed into a slot rect (an isometric cube for `BlockCube`, a flat tile
//! billboard for `Sprite`) plus the tiny bitmap-font count drawn over it. Shared by
//! every screen's draw — icons land in [`super::UiBuild`]'s reused buffers.

use glam::{Mat4, Vec3};

use super::super::block_model::{block_icon_faces, push_billboard_quad, push_cube_faces};
use super::super::chest_model::push_chest_item_full;
use super::super::ui_text;
use super::{pixel_to_ndc, push_solid, SlotRect, UiBuild, UiVertex};
use crate::block::Block;
use crate::item::{ItemRenderKind, ItemType};

/// Append the model3d icon for `item` placed into slot pixel rect `r`. Geometry is
/// appended into the shared, reused `build.icon_verts`/`icon_indices` buffers (no
/// per-icon allocation); the recorded [`super::IconDraw`] holds the sub-range + MVP.
pub(super) fn push_slot_icon(build: &mut UiBuild, screen: (u32, u32), item: ItemType, r: SlotRect) {
    let vert_start = build.icon_verts.len() as u32;
    let index_start = build.icon_indices.len() as u32;
    let mvp = match item.render_kind() {
        ItemRenderKind::BlockCube(block) => {
            // Unit cube centered on the origin, drawn with an isometric ortho MVP.
            // Per-face tiles so directional blocks (the furnace) show their front on
            // one face rather than all four. The chest instead draws its full inset
            // 3D model (body + lid + latch) so the icon matches the placed block.
            if block == Block::Chest {
                push_chest_item_full(
                    &mut build.icon_verts,
                    &mut build.icon_indices,
                    Vec3::splat(-0.5),
                    1.0,
                );
            } else {
                push_cube_faces(
                    &mut build.icon_verts,
                    &mut build.icon_indices,
                    block_icon_faces(block),
                    Vec3::splat(-0.5),
                    1.0,
                );
            }
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
    build.icons.push(super::IconDraw {
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
    pixel_to_ndc(screen, r.x + r.w * 0.5, r.y + r.h * 0.5)
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
pub(super) fn push_count(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    count: u32,
    r: SlotRect,
    scale: f32,
) {
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
    use super::super::gui_scale;
    use super::super::inventory::slot_rect;
    use super::*;

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
}
