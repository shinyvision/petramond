//! Extruded 3D mesh for a flat item sprite (flowers / tools), shared by the
//! first-person hand, the third-person held item, and the dropped item-entity.
//!
//! A flat 16×16 item tile is given real voxel depth by extruding its alpha mask:
//! a textured FRONT and BACK face (the full tile, alpha-cutout in the shader)
//! separated by a small depth, plus SIDE-WALL quads along every alpha BOUNDARY
//! edge — an opaque texel adjacent to a transparent texel or the tile border —
//! so the stepped silhouette gains thickness. Walls
//! are textured with that boundary texel's own sub-UV sampled from the block
//! atlas, which the `model3d` packed-vertex shader cannot do
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
use crate::atlas::tile_uv;
use glam::{Mat4, Vec3};
use petramond_math::face::Face;
use petramond_mesh::SHADES;
use petramond_world::bbmodel::face_corners;
use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::tile::Tile;
use petramond_world::tile_alpha::tile_alpha_opaque;

/// Bake a bbmodel block's baked model into indexed [`ItemVertex`] geometry (sampling the
/// MODEL atlas, the same sheet the in-world block uses) — the model centred + uniformly
/// scaled to a unit cube (`±0.5`), then placed by `transform`, lit by the two-channel
/// `light` under `env` (folded into the vertex TINT as an RGB factor; the vertex `shade`
/// keeps only the directional term). APPENDS (caller clears).
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
    // invariant and carries its own colour) folds into the tint; `shade` keeps
    // the directional term only.
    let tint = lighting::fold_tint([1.0, 1.0, 1.0], light, env);

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
            * Mat4::from_quat(petramond_world::bbmodel::euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            // Faces the chunk bake dropped (fully transparent atlas rect) drop
            // here too — same faces in every presentation.
            if !inst.face_draw[ci][slot] {
                continue;
            }
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
            // The same startup-baked self-AO the chunk mesh shades with, so the
            // held/dropped model matches the placed one.
            let ao = inst.face_ao[ci][slot];
            // Half-texel-inset against edge-texel spill, like the chunk bake.
            let [u0, v0, u1, v1] = block_model::atlas().inset_face_uv(uv);
            let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
            let start = verts.len() as u32;
            for i in 0..4 {
                verts.push(ItemVertex {
                    pos: p[i].to_array(),
                    uv: corner_uv[i],
                    shade: shade * ao[i],
                    tint,
                });
            }
            indices.extend(block_model::model_face_tris(ao).map(|i| start + i));
        }
    }
}

/// Bake a bbmodel block's model into [`ItemVertex`] geometry for the inventory-icon pass:
/// like [`build_block_model_item`] but `transform` is the full icon clip-space MVP (so
/// positions come out in clip space, ready for the pass-through `model_icon` shader). The
/// model-icon pass is DEPTH-BUFFERED (depth — not winding — orders its panels/drawers),
/// but the faces are also emitted FAR→NEAR by clip-z as a cheap, stable tiebreak for
/// coincident decals. Full-bright (no block light); APPENDS (caller clears).
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
    let mut faces: Vec<(f32, [ItemVertex; 4], [u32; 6])> = Vec::new();
    for (ci, cube) in inst.cubes.iter().enumerate() {
        let m = map
            * Mat4::from_translation(cube.origin)
            * Mat4::from_quat(petramond_world::bbmodel::euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            if !inst.face_draw[ci][slot] {
                continue;
            }
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
            // Icons carry the same baked self-AO as the placed/held model.
            let ao = inst.face_ao[ci][slot];
            // Half-texel-inset against edge-texel spill, like the chunk bake.
            let [u0, v0, u1, v1] = block_model::atlas().inset_face_uv(uv);
            let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
            let corner = |i: usize| ItemVertex {
                pos: p[i].to_array(),
                uv: corner_uv[i],
                shade: shade * ao[i],
                tint,
            };
            let quad = [corner(0), corner(1), corner(2), corner(3)];
            let depth = (p[0].z + p[1].z + p[2].z + p[3].z) * 0.25;
            faces.push((depth, quad, block_model::model_face_tris(ao)));
        }
    }
    // Larger clip-z is farther (wgpu z in [0,1], 0 = near): draw it FIRST so nearer faces
    // overpaint it.
    faces.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, quad, tris) in faces {
        let start = verts.len() as u32;
        verts.extend_from_slice(&quad);
        indices.extend(tris.map(|i| start + i));
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
/// Extrusion depth as a fraction of the 1.0 sprite size: exactly one texel
/// (1/16 of a full block width), so the extruded shape is pixel-perfect — its
/// side walls are precisely one texel wide and one texel deep.
const DEPTH: f32 = 1.0 / 16.0;

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

/// Apply a stack's `petramond:tint` to already-built extruded-sprite verts:
/// multiply the tint in and shift the UVs into the atlas's dye-base half
/// (desaturated, peak-white twins), so the tint can both dye and whiten.
/// No-op for a plain stack.
pub(super) fn dye_item_verts(verts: &mut [ItemVertex], variant: petramond_world::item::VariantId) {
    let Some(t) = petramond_world::item::variant::tint(variant) else {
        return;
    };
    for v in verts.iter_mut() {
        v.tint = [v.tint[0] * t[0], v.tint[1] * t[1], v.tint[2] * t[2]];
        v.uv[1] += crate::atlas::DYE_V_OFFSET;
    }
}

/// The packed-[`Vertex`] twin of [`dye_item_verts`]: apply a stack's
/// `petramond:tint` to already-built block verts (held mini-cube, dropped
/// cube, third-person hand) — multiply the tint in and set
/// [`petramond_mesh::DYED_FLAG2`] so the shader samples the dye-base twin.
/// No-op for a plain stack. Every `Vertex` dye path routes through here so
/// no caller can multiply without the flag (or vice versa).
///
/// [`Vertex`]: petramond_mesh::Vertex
pub(super) fn dye_block_verts(
    verts: &mut [petramond_mesh::Vertex],
    variant: petramond_world::item::VariantId,
) {
    let Some(t) = petramond_world::item::variant::tint(variant) else {
        return;
    };
    for v in verts.iter_mut() {
        let base = petramond_mesh::unpack_tint(v.tint);
        // `retint`, not `pack_tint`: the tint word's alpha lane carries the block
        // light's chroma, and rebuilding the word from scratch would erase it.
        v.tint = petramond_mesh::retint(v.tint, [base[0] * t[0], base[1] * t[1], base[2] * t[2]]);
        v.packed2 |= petramond_mesh::DYED_FLAG2;
    }
}

#[cfg(test)]
pub fn build_extruded_item(tile: Tile, out: &mut Vec<ItemVertex>) -> u32 {
    build_extruded_item_lit(tile, DynLight::FULL, LightEnv::IDENTITY, out)
}

/// Build the extruded held-item mesh for `tile` into `out` (cleared first,
/// capacity reused — no growth once warmed). Returns the vertex count. The mesh
/// is a non-indexed triangle list (the item3d pipeline draws it with `draw`).
///
/// FRONT/BACK are the full tile (alpha-cutout in the shader); side walls are
/// emitted per alpha-boundary texel edge with that texel's own sub-UV.
pub(super) fn build_extruded_item_lit(
    tile: Tile,
    light: DynLight,
    env: LightEnv,
    out: &mut Vec<ItemVertex>,
) -> u32 {
    // The GEOMETRY depends on nothing but the tile — light and foliage tint
    // only scale the per-vertex colour — so it is built once per tile and
    // re-tinted after. The scan below issues five alpha probes per texel over a
    // 16x16 grid; a mod draw set may ask for several sprite prims EVERY FRAME,
    // and rebuilding them was three quarters of the whole draw-set frame cost.
    let tint = lighting::fold_tint(foliage_tint::face_material(tile).tint, light, env);
    SPRITE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let geom = cache
            .entry(tile)
            .or_insert_with(|| build_extruded_item_geometry(tile));
        out.clear();
        out.extend(geom.iter().map(|v| ItemVertex {
            tint: [
                v.tint[0] * tint[0],
                v.tint[1] * tint[1],
                v.tint[2] * tint[2],
            ],
            ..*v
        }));
        out.len() as u32
    })
}

thread_local! {
    /// Per-tile extruded sprite geometry, white-tinted. Thread-local rather
    /// than shared: only the render thread builds these, so this costs no lock.
    /// Never invalidated, and does not need to be: `Tile` indexes the
    /// process-wide atlas, which is a `LazyLock` built once.
    static SPRITE_CACHE: std::cell::RefCell<rustc_hash::FxHashMap<Tile, Vec<ItemVertex>>> =
        std::cell::RefCell::new(rustc_hash::FxHashMap::default());
}

/// How a `petramond:overlay` slab floats off the base slab it decorates: a
/// Build the full extruded mesh a STACK shows for its sprite `tile`: the base
/// sprite with its `petramond:overlay` items COMPOSITED over it, baked at
/// runtime into ONE slab, then dyed (base texels only — a dyed tool tints its
/// body, never the augment riding on it). Clears `out`; returns the vertex
/// count. Every world-space sprite presentation (both hands, dropped
/// entities) must build through this rather than the bare slab, or augmented
/// items silently lose their overlay there.
///
/// WHY a baked composite and not a second slab: any second slab has to float
/// off the first to avoid z-fighting, and floating means its texels leave the
/// base's pixel grid — visibly misaligned at 16 px (Rachel, 2026-08-05). The
/// composite is one slab on one grid, pixel-perfect by construction. And
/// because per-texel compositing never BLENDS — every composited texel is
/// owned outright by the topmost opaque layer — the bake needs no new
/// texture: each texel run samples its OWNER tile's own atlas rect, so mips,
/// the dye-base half, and the one-atlas draw batching all keep working.
pub(super) fn build_extruded_stack_lit(
    tile: Tile,
    variant: petramond_world::item::VariantId,
    light: DynLight,
    env: LightEnv,
    out: &mut Vec<ItemVertex>,
) -> u32 {
    let overlay_tiles: Vec<Tile> = petramond_world::item::variant::overlay_items(variant)
        .into_iter()
        .filter_map(|item| match item.render_kind() {
            petramond_world::item::ItemRenderKind::Sprite(t) => Some(t),
            _ => None,
        })
        .collect();
    if overlay_tiles.is_empty() {
        let count = build_extruded_item_lit(tile, light, env, out);
        if count == 0 {
            return 0;
        }
        dye_item_verts(out, variant);
        return out.len() as u32;
    }

    // Per-tile foliage tint is baked into the cached geometry (it is a
    // property of each OWNER tile); only the light fold is per-call.
    let fold = lighting::fold_tint([1.0, 1.0, 1.0], light, env);
    out.clear();
    COMPOSITE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let geom = cache
            .entry((tile, overlay_tiles.clone()))
            .or_insert_with(|| build_composited_geometry(tile, &overlay_tiles));
        let lit = |v: &ItemVertex| ItemVertex {
            tint: [
                v.tint[0] * fold[0],
                v.tint[1] * fold[1],
                v.tint[2] * fold[2],
            ],
            ..*v
        };
        out.extend(geom.base.iter().map(lit));
        // The dye reaches exactly the BASE-owned geometry: the split is why
        // the cache keeps two vecs instead of one.
        dye_item_verts(out, variant);
        out.extend(geom.over.iter().map(lit));
    });
    out.len() as u32
}

/// Cached composited-slab geometry, split by texel OWNER so the stack's dye
/// can reach the base sprite's geometry and nothing else.
struct CompositeGeometry {
    base: Vec<ItemVertex>,
    over: Vec<ItemVertex>,
}

thread_local! {
    /// Per-(base, overlays) composited slab geometry, foliage-tinted, unlit.
    /// Thread-local like [`SPRITE_CACHE`], and bounded by the augment combos
    /// that actually appear in a session.
    static COMPOSITE_CACHE: std::cell::RefCell<
        rustc_hash::FxHashMap<(Tile, Vec<Tile>), CompositeGeometry>,
    > = std::cell::RefCell::new(rustc_hash::FxHashMap::default());
}

/// The composited slab for `tile` under `overlays` (in declared order,
/// later on top): one extrusion over the COMPOSITE alpha, every face
/// sampling the atlas rect of the texel's owning tile.
fn build_composited_geometry(tile: Tile, overlays: &[Tile]) -> CompositeGeometry {
    // The texel's owner: the topmost opaque overlay, else the opaque base,
    // else nothing. Off-grid coordinates own nothing (wall probes).
    let owner = |tx: i32, ty: i32| -> Option<Tile> {
        if !(0..GRID as i32).contains(&tx) || !(0..GRID as i32).contains(&ty) {
            return None;
        }
        for &o in overlays.iter().rev() {
            if opaque(o, tx, ty) {
                return Some(o);
            }
        }
        opaque(tile, tx, ty).then_some(tile)
    };
    let mut geom = CompositeGeometry {
        base: Vec::new(),
        over: Vec::new(),
    };
    let zf = DEPTH * 0.5;
    let zb = -DEPTH * 0.5;

    // FRONT + BACK faces: per row, greedy runs of texels sharing one owner —
    // each run is one quad over the owner's own atlas sub-rect, so the
    // composite is sampled where its pixels actually live.
    for ty in 0..GRID as i32 {
        let mut tx = 0;
        while tx < GRID as i32 {
            let Some(own) = owner(tx, ty) else {
                tx += 1;
                continue;
            };
            let start = tx;
            while tx < GRID as i32 && owner(tx, ty) == Some(own) {
                tx += 1;
            }
            let tint = foliage_tint::face_material(own).tint;
            let out = if own == tile {
                &mut geom.base
            } else {
                &mut geom.over
            };
            let [su0, sv0, _, _] = texel_uv_rect(own, start, ty);
            let [_, _, eu1, ev1] = texel_uv_rect(own, tx - 1, ty);
            let (xl, xr) = (px(start), px(tx));
            let (yt, yb) = (py(ty), py(ty + 1));
            push_quad(
                out,
                [[xl, yb, zf], [xr, yb, zf], [xr, yt, zf], [xl, yt, zf]],
                [[su0, ev1], [eu1, ev1], [eu1, sv0], [su0, sv0]],
                SHADE_FRONT,
                tint,
            );
            push_quad(
                out,
                [[xr, yb, zb], [xl, yb, zb], [xl, yt, zb], [xr, yt, zb]],
                [[eu1, ev1], [su0, ev1], [su0, sv0], [eu1, sv0]],
                SHADE_BACK,
                tint,
            );
        }
    }

    // SIDE WALLS: on the COMPOSITE's alpha boundary, each wall textured with
    // the owning texel's own single-texel patch (its centre UV), exactly like
    // the plain extrusion's rim.
    for ty in 0..GRID as i32 {
        for tx in 0..GRID as i32 {
            let Some(own) = owner(tx, ty) else {
                continue;
            };
            let tint = foliage_tint::face_material(own).tint;
            let out = if own == tile {
                &mut geom.base
            } else {
                &mut geom.over
            };
            let [tu0, tv0, tu1, tv1] = texel_uv_rect(own, tx, ty);
            let uc = [(tu0 + tu1) * 0.5, (tv0 + tv1) * 0.5];
            let xl = px(tx);
            let xr = px(tx + 1);
            let yt = py(ty);
            let yb = py(ty + 1);
            if owner(tx - 1, ty).is_none() {
                push_quad(
                    out,
                    [[xl, yb, zb], [xl, yb, zf], [xl, yt, zf], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            if owner(tx + 1, ty).is_none() {
                push_quad(
                    out,
                    [[xr, yb, zf], [xr, yb, zb], [xr, yt, zb], [xr, yt, zf]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            if owner(tx, ty - 1).is_none() {
                push_quad(
                    out,
                    [[xl, yt, zf], [xr, yt, zf], [xr, yt, zb], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            if owner(tx, ty + 1).is_none() {
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

    geom
}

/// [`build_extruded_item_lit`]'s geometry, untinted.
fn build_extruded_item_geometry(tile: Tile) -> Vec<ItemVertex> {
    let mut verts = Vec::new();
    let out = &mut verts;

    // WHITE, deliberately: the foliage tint (grass-green for a fern, white for
    // everything else) and the RGB light fold are both a per-vertex multiply,
    // so the caller applies them to this cached geometry instead. `shade`,
    // which is per-FACE, is baked in below.
    let tint = [1.0, 1.0, 1.0];
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

    verts
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

    /// The composited slab is ONE extrusion on ONE pixel grid — the invariant
    /// the floating-second-slab approach broke (misaligned overlay texels,
    /// 2026-08-05). Every coordinate sits exactly on the 1/16 texel grid at
    /// the plain slab's own depth, and both owners' geometry samples its OWN
    /// tile's atlas rect.
    #[test]
    fn composited_slab_stays_on_the_pixel_grid_and_samples_both_owners() {
        let base = Tile::named("stone_pickaxe");
        let over = Tile::named("diamond");
        let geom = build_composited_geometry(base, &[over]);
        assert!(!geom.base.is_empty(), "base-owned texels remain visible");
        assert!(!geom.over.is_empty(), "overlay-owned texels exist");
        let on_grid = |v: f32| {
            let scaled = (v + 0.5) * GRID as f32;
            (scaled - scaled.round()).abs() < 1e-4
        };
        let in_rect = |uv: [f32; 2], r: [f32; 4]| {
            uv[0] >= r[0] - 1e-4
                && uv[0] <= r[2] + 1e-4
                && uv[1] >= r[1] - 1e-4
                && uv[1] <= r[3] + 1e-4
        };
        let (br, or) = (tile_uv(base), tile_uv(over));
        for v in geom.base.iter().chain(&geom.over) {
            assert!(
                on_grid(v.pos[0]) && on_grid(v.pos[1]),
                "off-grid: {:?}",
                v.pos
            );
            assert!(
                (v.pos[2].abs() - DEPTH * 0.5).abs() < 1e-5,
                "one slab, one depth: {:?}",
                v.pos
            );
        }
        assert!(
            geom.base.iter().all(|v| in_rect(v.uv, br)),
            "base geometry samples the base tile only"
        );
        assert!(
            geom.over.iter().all(|v| in_rect(v.uv, or)),
            "overlay geometry samples the overlay tile only"
        );
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
            DynLight {
                sky: 0,
                block: petramond_world::light::BlockLight6::DARK,
            },
            LightEnv::IDENTITY,
            &mut out,
        );

        assert_eq!(out[0].shade, SHADE_FRONT);
        let dark = lighting::light_rgb(
            DynLight {
                sky: 0,
                block: petramond_world::light::BlockLight6::DARK,
            },
            LightEnv::IDENTITY,
        );
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
