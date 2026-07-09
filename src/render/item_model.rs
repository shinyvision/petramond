//! Extruded 3D mesh for a held flat item sprite (flowers / future tools).
//!
//! A flat 16×16 item tile is given real voxel depth by extruding its alpha mask:
//! a textured FRONT and BACK face (the full tile, alpha-cutout in the shader)
//! separated by a small depth, plus SIDE-WALL quads along every alpha BOUNDARY
//! edge — an opaque texel adjacent to a transparent texel or the tile border —
//! so the stepped silhouette gains thickness. Walls
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
use super::lighting::{self, DynLight, LightEnv};
use crate::atlas::{tile_alpha_opaque, tile_uv, Tile};
use crate::bbmodel::face_corners;
use crate::block_model::{self, BlockModelKind};
use crate::mesh::face::Face;
use crate::mesh::SHADES;
use glam::{Mat4, Vec3};

/// Bake a bbmodel block's baked model into indexed [`ItemVertex`] geometry (sampling the
/// MODEL atlas, the same sheet the in-world block uses) — the model centred + uniformly
/// scaled to a unit cube (`±0.5`), then placed by `transform`, lit by the two-channel
/// `light` under `env` (folded into the vertex TINT as an RGB factor; the vertex `shade`
/// keeps only the directional term) and warmed by `warm` (0..255). APPENDS (caller
/// clears).
/// Shared by the inventory ICON, the first-person HELD item, and the DROPPED item-entity
/// so all three show the real workbench, not a stand-in cube.
///
/// `view_sort`, when `Some(dir)`, orders the cubes far→near along `dir` so a DEPTHLESS
/// pass (the iso inventory icon) gets correct overlap by painter's algorithm; the
/// depth-tested hand/world contexts pass `None`.
pub fn build_block_model_item(
    kind: BlockModelKind,
    transform: Mat4,
    light: DynLight,
    env: LightEnv,
    warm: u8,
    view_sort: Option<Vec3>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    let inst = block_model::instance(kind);
    let fp = Vec3::new(
        inst.footprint[0] as f32,
        inst.footprint[1] as f32,
        inst.footprint[2] as f32,
    );
    // Footprint space → a unit cube centred on the origin: subtract the footprint centre,
    // then uniformly scale the largest axis to fill `±0.5` (keeping proportions). The
    // caller's `transform` then sizes/places/spins it for its context.
    let span = fp.max_element().max(1.0);
    let map =
        transform * Mat4::from_scale(Vec3::splat(1.0 / span)) * Mat4::from_translation(-fp * 0.5);
    // RGB light (sky channel dims/tints with the env; block channel is night-
    // invariant) folds into the tint; `shade` keeps the directional term only.
    let rgb = lighting::light_rgb(light, env);
    let warm = crate::torch::warm_tint([1.0, 1.0, 1.0], warm as f32 / 255.0);
    let tint = [warm[0] * rgb[0], warm[1] * rgb[1], warm[2] * rgb[2]];

    // Draw order (far→near for the depthless icon; natural otherwise).
    let mut order: Vec<usize> = (0..inst.cubes.len()).collect();
    if let Some(dir) = view_sort {
        order.sort_by(|&a, &b| {
            let da = ((inst.cubes[a].from + inst.cubes[a].to) * 0.5).dot(dir);
            let db = ((inst.cubes[b].from + inst.cubes[b].to) * 0.5).dot(dir);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    for &ci in &order {
        let cube = &inst.cubes[ci];
        let m = map
            * Mat4::from_translation(cube.origin)
            * Mat4::from_quat(crate::bbmodel::euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            let Some(bias) = block_model::render_face_bias(cube, &inst.cubes, face) else {
                continue;
            };
            let local = face_corners(face, cube.from, cube.to);
            let p: [Vec3; 4] = [
                m.transform_point3(Vec3::from(local[0]) + bias),
                m.transform_point3(Vec3::from(local[1]) + bias),
                m.transform_point3(Vec3::from(local[2]) + bias),
                m.transform_point3(Vec3::from(local[3]) + bias),
            ];
            if (p[1] - p[0]).cross(p[3] - p[0]).length_squared() < 1e-12 {
                continue;
            }
            let shade = SHADES[face.shade_idx() as usize];
            let [u0, v0, u1, v1] = uv;
            let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
            let start = verts.len() as u32;
            for i in 0..4 {
                verts.push(ItemVertex {
                    pos: p[i].to_array(),
                    uv: corner_uv[i],
                    shade,
                    tint,
                });
            }
            indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        }
    }
}

/// Bake a bbmodel block's model into [`ItemVertex`] geometry for the inventory-icon pass:
/// like [`build_block_model_item`] but `transform` is the full icon clip-space MVP (so
/// positions come out in clip space, ready for the pass-through `model_icon` shader). The
/// model-icon pass is DEPTH-BUFFERED (the model is double-sided like the in-world block,
/// so depth — not winding — orders its panels/drawers), but the faces are also emitted
/// FAR→NEAR by clip-z as a cheap, stable tiebreak for coincident decals. Full-bright (no
/// warm tint); APPENDS (caller clears).
pub fn build_block_model_icon(
    kind: BlockModelKind,
    mvp: Mat4,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    let inst = block_model::instance(kind);
    let fp = Vec3::new(
        inst.footprint[0] as f32,
        inst.footprint[1] as f32,
        inst.footprint[2] as f32,
    );
    // Footprint space → centred unit cube (same as `build_block_model_item`), then the
    // caller's icon MVP — so positions land in clip space.
    let span = fp.max_element().max(1.0);
    let map = mvp * Mat4::from_scale(Vec3::splat(1.0 / span)) * Mat4::from_translation(-fp * 0.5);
    // Full-bright, and always at the identity environment: icons are UI, not world.
    let light = lighting::light_rgb(DynLight::FULL, LightEnv::IDENTITY)[0];
    let tint = [1.0, 1.0, 1.0];

    // Collect every face with its mean clip-z, then sort far→near (painter's algorithm).
    let mut faces: Vec<(f32, [ItemVertex; 4])> = Vec::new();
    for cube in &inst.cubes {
        let m = map
            * Mat4::from_translation(cube.origin)
            * Mat4::from_quat(crate::bbmodel::euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            let Some(bias) = block_model::render_face_bias(cube, &inst.cubes, face) else {
                continue;
            };
            let local = face_corners(face, cube.from, cube.to);
            let p: [Vec3; 4] = [
                m.transform_point3(Vec3::from(local[0]) + bias),
                m.transform_point3(Vec3::from(local[1]) + bias),
                m.transform_point3(Vec3::from(local[2]) + bias),
                m.transform_point3(Vec3::from(local[3]) + bias),
            ];
            if (p[1] - p[0]).cross(p[3] - p[0]).length_squared() < 1e-12 {
                continue;
            }
            let shade = SHADES[face.shade_idx() as usize] * light;
            let [u0, v0, u1, v1] = uv;
            let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
            let quad = [
                ItemVertex {
                    pos: p[0].to_array(),
                    uv: corner_uv[0],
                    shade,
                    tint,
                },
                ItemVertex {
                    pos: p[1].to_array(),
                    uv: corner_uv[1],
                    shade,
                    tint,
                },
                ItemVertex {
                    pos: p[2].to_array(),
                    uv: corner_uv[2],
                    shade,
                    tint,
                },
                ItemVertex {
                    pos: p[3].to_array(),
                    uv: corner_uv[3],
                    shade,
                    tint,
                },
            ];
            let depth = (p[0].z + p[1].z + p[2].z + p[3].z) * 0.25;
            faces.push((depth, quad));
        }
    }
    // Larger clip-z is farther (wgpu z in [0,1], 0 = near): draw it FIRST so nearer faces
    // overpaint it.
    faces.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, quad) in faces {
        let start = verts.len() as u32;
        verts.extend_from_slice(&quad);
        indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
    }
}

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
    build_extruded_item_lit(tile, DynLight::FULL, LightEnv::IDENTITY, out)
}

pub(super) fn build_extruded_item_lit(
    tile: Tile,
    light: DynLight,
    env: LightEnv,
    out: &mut Vec<ItemVertex>,
) -> u32 {
    out.clear();

    // Foliage tint for the whole sprite: grass-green for a held fern / short grass,
    // white (no-op) for flowers / tools / blocks. The fern tile is grayscale and
    // would read gray in-hand without this — matches the dropped-item + icon paths
    // (both via `foliage_tint::face_material`).
    // RGB light folds into the tint (see `build_block_model_item`); `shade` keeps
    // the front/back/side directional terms only.
    let rgb = lighting::light_rgb(light, env);
    let base = foliage_tint::face_material(tile).tint;
    let tint = [base[0] * rgb[0], base[1] * rgb[1], base[2] * rgb[2]];
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
        SHADE_FRONT,
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
        SHADE_BACK,
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
                    SHADE_SIDE,
                    tint,
                );
            }
            // RIGHT edge wall (neighbour tx+1 transparent): plane x = xr.
            if !opaque(tile, tx + 1, ty) {
                push_quad(
                    out,
                    [[xr, yb, zf], [xr, yb, zb], [xr, yt, zb], [xr, yt, zf]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            // TOP edge wall (neighbour ty-1 transparent): plane y = yt.
            if !opaque(tile, tx, ty - 1) {
                push_quad(
                    out,
                    [[xl, yt, zf], [xr, yt, zf], [xr, yt, zb], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            // BOTTOM edge wall (neighbour ty+1 transparent): plane y = yb.
            if !opaque(tile, tx, ty + 1) {
                push_quad(
                    out,
                    [[xl, yb, zb], [xr, yb, zb], [xr, yb, zf], [xl, yb, zf]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
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
        let n = build_extruded_item(Tile::named("poppy"), &mut out);
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
        build_extruded_item(Tile::named("poppy"), &mut out);
        let [u0, v0, u1, v1] = tile_uv(Tile::named("poppy"));
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
        build_extruded_item(Tile::named("poppy"), &mut out);
        let cap = out.capacity();
        // Same tile -> identical vert count -> capacity unchanged.
        build_extruded_item(Tile::named("poppy"), &mut out);
        assert_eq!(
            out.capacity(),
            cap,
            "rebuild must reuse the buffer capacity"
        );
    }

    #[test]
    fn lit_extruded_item_folds_light_into_the_tint() {
        // The two-channel RGB light rides the vertex TINT (shade keeps only the
        // directional term), so a dark sample dims the tint, not the shade.
        let mut out = Vec::new();
        build_extruded_item_lit(
            Tile::named("poppy"),
            DynLight { sky: 0, block: 0 },
            LightEnv::IDENTITY,
            &mut out,
        );

        assert_eq!(out[0].shade, SHADE_FRONT);
        let dark = lighting::light_rgb(DynLight { sky: 0, block: 0 }, LightEnv::IDENTITY);
        assert_eq!(out[0].tint, dark, "unlit sample dims the tint");
        assert!(dark[0] < 1.0);
    }

    #[test]
    fn solid_alpha_tile_has_only_border_walls() {
        // A fully-opaque tile (Stone) extrudes to front + back + a wall on each of
        // the 4 outer borders only (16 texels per border edge): no interior walls.
        let mut out = Vec::new();
        build_extruded_item(Tile::named("stone"), &mut out);
        // 2 faces * 6 + 4 borders * 16 texels * 6 verts = 12 + 384 = 396.
        assert_eq!(out.len(), 12 + 4 * GRID * 6);
    }
}
