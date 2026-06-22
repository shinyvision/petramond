//! Extruded 3D mesh for a held flat item sprite (flowers / future tools).
//!
//! A flat 16×16 item tile is given real voxel depth by extruding its alpha mask:
//! a textured FRONT and BACK face (the full tile, alpha-cutout in the shader)
//! separated by a small depth, plus SIDE-WALL quads along every alpha BOUNDARY
//! edge — an opaque texel adjacent to a transparent texel or the tile border —
//! so the stepped silhouette gains thickness like a Minecraft item entity. Walls
//! are textured with that boundary texel's own sub-UV sampled from the block
//! atlas, which the [`model3d`](super::model3d) packed-vertex shader cannot do
//! (it can only SELECT whole-tile UV corners), so this drives the dedicated
//! `item3d` pipeline + shader with EXPLICIT per-vertex `(pos, uv, shade)`.
//!
//! The mesh is built in a unit, origin-centred model space: `x`/`y` in
//! `[-0.5, 0.5]` (the 16×16 sprite), `z` the extrusion (`+depth/2` front,
//! `-depth/2` back). The caller ([`super::hand`]) applies the held-angle model
//! matrix. Full-bright; each face carries a directional `shade` so the depth
//! reads (front brightest, back dim, side walls mid).

use super::foliage_tint;
use super::lighting;
use crate::atlas::{tile_alpha_opaque, tile_uv, Tile};

/// One vertex of the extruded item mesh consumed by the `item3d` pipeline:
/// explicit position, atlas UV, a directional shade multiplier, and an RGB tint
/// (foliage-green for a held fern / short grass, white otherwise — the grayscale
/// fern tile would read gray without it, same as the icon / dropped-item paths).
/// `#[repr(C)]` + `bytemuck` so the renderer can upload it straight to the GPU;
/// the vertex layout (pos f32x3 @0, uv f32x2 @12, shade f32 @20, tint f32x3 @24) is
/// declared in `pipeline.rs` and mirrored by `item3d.wgsl`'s `VsIn`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ItemVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub tint: [f32; 3],
}

/// Texels per side of an item tile (the alpha mask is sampled on this grid).
const GRID: usize = 16;
/// Extrusion depth as a fraction of the 1.0 sprite size (~1.5/16, MC-like).
const DEPTH: f32 = 1.5 / 16.0;

/// Directional shades so the extrusion reads as 3D (front brightest, back dim,
/// side walls in between). Mirrors the "top bright / bottom dark" voxel feel.
const SHADE_FRONT: f32 = 1.0;
const SHADE_BACK: f32 = 0.6;
const SHADE_SIDE: f32 = 0.8;

/// Is texel `(tx, ty)` (ty top-down, matching the atlas alpha rows) opaque under
/// the cutout test? Texels outside the grid count as transparent (the border),
/// so edge-of-tile opaque texels still get a side wall.
#[inline]
fn opaque(tile: Tile, tx: i32, ty: i32) -> bool {
    if tx < 0 || ty < 0 || tx >= GRID as i32 || ty >= GRID as i32 {
        return false;
    }
    // tile_alpha_opaque takes (u, v_bottom_up). Texel centre: u = (tx+0.5)/16,
    // and the alpha rows are top-down so v_bottom_up = 1 - (ty+0.5)/16.
    let u = (tx as f32 + 0.5) / GRID as f32;
    let v_bottom_up = 1.0 - (ty as f32 + 0.5) / GRID as f32;
    tile_alpha_opaque(tile, u, v_bottom_up)
}

/// Atlas UV of texel `(tx, ty)` (ty top-down) within `tile`'s rect: returns
/// `(u0, v0, u1, v1)` for that single texel, where v0 is the TOP edge in atlas
/// space (atlas v increases downward) so it composes with `corner` ordering.
#[inline]
fn texel_uv_rect(tile: Tile, tx: i32, ty: i32) -> [f32; 4] {
    let [u0, v0, u1, v1] = tile_uv(tile);
    let du = (u1 - u0) / GRID as f32;
    let dv = (v1 - v0) / GRID as f32;
    let tu0 = u0 + du * tx as f32;
    let tv0 = v0 + dv * ty as f32;
    [tu0, tv0, tu0 + du, tv0 + dv]
}

/// Model-space X for texel column `tx` left edge (`tx` in `0..=16`), centred:
/// column 0 → -0.5, column 16 → +0.5.
#[inline]
fn px(tx: i32) -> f32 {
    tx as f32 / GRID as f32 - 0.5
}

/// Model-space Y for texel row `ty` (ty top-down, `0..=16`): row 0 (top) → +0.5,
/// row 16 (bottom) → -0.5, so the sprite is upright.
#[inline]
fn py(ty: i32) -> f32 {
    0.5 - ty as f32 / GRID as f32
}

#[inline]
fn push_quad(
    out: &mut Vec<ItemVertex>,
    corners: [[f32; 3]; 4],
    uvs: [[f32; 2]; 4],
    shade: f32,
    tint: [f32; 3],
) {
    // Two triangles (0,1,2)(0,2,3). The item3d pipeline disables back-face cull,
    // so winding need not be consistent across the mixed front/back/wall faces.
    for &i in &[0usize, 1, 2, 0, 2, 3] {
        out.push(ItemVertex {
            pos: corners[i],
            uv: uvs[i],
            shade,
            tint,
        });
    }
}

/// Build the extruded held-item mesh for `tile` into `out` (cleared first,
/// capacity reused — no growth once warmed). Returns the vertex count. The mesh
/// is a non-indexed triangle list (the item3d pipeline draws it with `draw`).
///
/// FRONT/BACK are the full tile (alpha-cutout in the shader); side walls are
/// emitted per alpha-boundary texel edge with that texel's own sub-UV.
#[cfg(test)]
pub fn build_extruded_item(tile: Tile, out: &mut Vec<ItemVertex>) -> u32 {
    build_extruded_item_lit(tile, lighting::FULL_SKYLIGHT, out)
}

pub(super) fn build_extruded_item_lit(tile: Tile, skylight: u8, out: &mut Vec<ItemVertex>) -> u32 {
    out.clear();

    // Foliage tint for the whole sprite: grass-green for a held fern / short grass,
    // white (no-op) for flowers / tools / blocks. The fern tile is grayscale and
    // would read gray in-hand without this — matches the dropped-item + icon paths
    // (both via `foliage_tint::face_material`).
    let tint = foliage_tint::face_material(tile).tint;
    let light = lighting::sky_light_factor(skylight);
    let zf = DEPTH * 0.5;
    let zb = -DEPTH * 0.5;
    let [fu0, fv0, fu1, fv1] = tile_uv(tile);

    // FRONT face (+Z), CCW seen from +Z. Corner order bl, br, tr, tl with UVs
    // matching: bottom-left = (u0, v1) since atlas v increases downward.
    push_quad(
        out,
        [
            [-0.5, -0.5, zf],
            [0.5, -0.5, zf],
            [0.5, 0.5, zf],
            [-0.5, 0.5, zf],
        ],
        [[fu0, fv1], [fu1, fv1], [fu1, fv0], [fu0, fv0]],
        SHADE_FRONT * light,
        tint,
    );
    // BACK face (-Z), wound the other way so it faces -Z.
    push_quad(
        out,
        [
            [0.5, -0.5, zb],
            [-0.5, -0.5, zb],
            [-0.5, 0.5, zb],
            [0.5, 0.5, zb],
        ],
        [[fu1, fv1], [fu0, fv1], [fu0, fv0], [fu1, fv0]],
        SHADE_BACK * light,
        tint,
    );

    // SIDE WALLS: for every opaque texel, emit a depth-spanning wall quad on each
    // of its 4 edges where the neighbour is transparent / off-tile. Each wall is
    // textured with the OWNING texel's sub-UV (a single texel patch) so the
    // stepped rim shows the sprite's colour at that pixel.
    for ty in 0..GRID as i32 {
        for tx in 0..GRID as i32 {
            if !opaque(tile, tx, ty) {
                continue;
            }
            let [tu0, tv0, tu1, tv1] = texel_uv_rect(tile, tx, ty);
            // Texel quad bounds in model space (left/right X, top/bottom Y).
            let xl = px(tx);
            let xr = px(tx + 1);
            let yt = py(ty); // top edge (larger Y)
            let yb = py(ty + 1); // bottom edge (smaller Y)
                                 // Single-texel UV; pick a representative corner UV (texel centre) per
                                 // wall vertex so the rim samples this texel's colour.
            let uc = [(tu0 + tu1) * 0.5, (tv0 + tv1) * 0.5];

            // LEFT edge wall (neighbour tx-1 transparent): plane x = xl spanning z.
            if !opaque(tile, tx - 1, ty) {
                push_quad(
                    out,
                    [[xl, yb, zb], [xl, yb, zf], [xl, yt, zf], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE * light,
                    tint,
                );
            }
            // RIGHT edge wall (neighbour tx+1 transparent): plane x = xr.
            if !opaque(tile, tx + 1, ty) {
                push_quad(
                    out,
                    [[xr, yb, zf], [xr, yb, zb], [xr, yt, zb], [xr, yt, zf]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE * light,
                    tint,
                );
            }
            // TOP edge wall (neighbour ty-1 transparent): plane y = yt.
            if !opaque(tile, tx, ty - 1) {
                push_quad(
                    out,
                    [[xl, yt, zf], [xr, yt, zf], [xr, yt, zb], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE * light,
                    tint,
                );
            }
            // BOTTOM edge wall (neighbour ty+1 transparent): plane y = yb.
            if !opaque(tile, tx, ty + 1) {
                push_quad(
                    out,
                    [[xl, yb, zb], [xr, yb, zb], [xr, yb, zf], [xl, yb, zf]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE * light,
                    tint,
                );
            }
        }
    }

    out.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extruded_item_has_front_back_and_walls() {
        let mut out = Vec::new();
        let n = build_extruded_item(Tile::Poppy, &mut out);
        assert_eq!(n as usize, out.len());
        // Front (6) + back (6) at minimum; a real flower sprite has a non-trivial
        // silhouette so there must be many side-wall verts on top.
        assert!(
            out.len() > 12,
            "expected front+back+walls, got {}",
            out.len()
        );
        // Every wall/face vertex stays within the unit, origin-centred box.
        for v in &out {
            assert!(v.pos[0] >= -0.5 - 1e-4 && v.pos[0] <= 0.5 + 1e-4);
            assert!(v.pos[1] >= -0.5 - 1e-4 && v.pos[1] <= 0.5 + 1e-4);
            assert!(v.pos[2].abs() <= DEPTH * 0.5 + 1e-4);
            // Front/back/side shades only.
            assert!(
                v.shade == SHADE_FRONT || v.shade == SHADE_BACK || v.shade == SHADE_SIDE,
                "unexpected shade {}",
                v.shade
            );
        }
    }

    #[test]
    fn front_and_back_faces_use_full_tile_uv() {
        let mut out = Vec::new();
        build_extruded_item(Tile::Poppy, &mut out);
        let [u0, v0, u1, v1] = tile_uv(Tile::Poppy);
        // First 6 verts = front face; they must span the full tile rect corners.
        let front = &out[..6];
        let us: Vec<f32> = front.iter().map(|v| v.uv[0]).collect();
        let vs: Vec<f32> = front.iter().map(|v| v.uv[1]).collect();
        assert!(us.iter().any(|&u| (u - u0).abs() < 1e-5));
        assert!(us.iter().any(|&u| (u - u1).abs() < 1e-5));
        assert!(vs.iter().any(|&v| (v - v0).abs() < 1e-5));
        assert!(vs.iter().any(|&v| (v - v1).abs() < 1e-5));
    }

    #[test]
    fn rebuild_reuses_capacity() {
        let mut out = Vec::new();
        build_extruded_item(Tile::Poppy, &mut out);
        let cap = out.capacity();
        // Same tile -> identical vert count -> capacity unchanged.
        build_extruded_item(Tile::Poppy, &mut out);
        assert_eq!(
            out.capacity(),
            cap,
            "rebuild must reuse the buffer capacity"
        );
    }

    #[test]
    fn lit_extruded_item_scales_shades() {
        let mut out = Vec::new();
        build_extruded_item_lit(Tile::Poppy, 0, &mut out);

        assert_eq!(out[0].shade, SHADE_FRONT * lighting::sky_light_factor(0));
        assert!(out[0].shade < SHADE_FRONT);
    }

    #[test]
    fn solid_alpha_tile_has_only_border_walls() {
        // A fully-opaque tile (Stone) extrudes to front + back + a wall on each of
        // the 4 outer borders only (16 texels per border edge): no interior walls.
        let mut out = Vec::new();
        build_extruded_item(Tile::Stone, &mut out);
        // 2 faces * 6 + 4 borders * 16 texels * 6 verts = 12 + 384 = 396.
        assert_eq!(out.len(), 12 + 4 * GRID * 6);
    }
}
