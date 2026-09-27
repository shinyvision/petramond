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
use petramond_mesh::face::FaceShading;
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
    let span = fp.max_element().max(1.0);
    let map =
        transform * Mat4::from_scale(Vec3::splat(1.0 / span)) * Mat4::from_translation(-fp * 0.5);
    let tint = lighting::fold_tint([1.0, 1.0, 1.0], light, env);

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
            let appearance = block_model::atlas().appearance(kind, uv.uv);
            let tint = std::array::from_fn(|a| {
                let light = if appearance.unlit { 1.0 } else { tint[a] };
                light * f32::from(appearance.tint[a]) / 255.0
            });
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
            let ao = inst.face_ao[ci][slot];
            let corner_uv = uv
                .with_uv(block_model::atlas().inset_face_uv(uv.uv))
                .corner_uv();
            let start = verts.len() as u32;
            for i in 0..4 {
                verts.push(ItemVertex {
                    pos: p[i].to_array(),
                    uv: corner_uv[i],
                    shade: appearance.shading(shade, ao[i]),
                    tint,
                });
            }
            indices.extend(block_model::model_face_tris(ao).map(|i| start + i));
        }
    }
}

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
    let span = fp.max_element().max(1.0);
    let map = mvp * Mat4::from_scale(Vec3::splat(1.0 / span)) * Mat4::from_translation(-fp * 0.5);
    let light = lighting::light_rgb(DynLight::FULL, LightEnv::IDENTITY)[0];
    let tint = [1.0, 1.0, 1.0];

    let mut faces: Vec<(f32, [ItemVertex; 4], [u32; 6])> = Vec::new();
    for (ci, cube) in inst.cubes.iter().enumerate() {
        let m = map
            * Mat4::from_translation(cube.origin)
            * Mat4::from_quat(petramond_world::bbmodel::euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            let appearance = block_model::atlas().appearance(kind, uv.uv);
            let tint = std::array::from_fn(|a| tint[a] * f32::from(appearance.tint[a]) / 255.0);
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
            let ao = inst.face_ao[ci][slot];
            let corner_uv = uv
                .with_uv(block_model::atlas().inset_face_uv(uv.uv))
                .corner_uv();
            let corner = |i: usize| ItemVertex {
                pos: p[i].to_array(),
                uv: corner_uv[i],
                shade: appearance.shading(shade, ao[i]),
                tint,
            };
            let quad = [corner(0), corner(1), corner(2), corner(3)];
            let depth = (p[0].z + p[1].z + p[2].z + p[3].z) * 0.25;
            faces.push((depth, quad, block_model::model_face_tris(ao)));
        }
    }
    faces.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, quad, tris) in faces {
        let start = verts.len() as u32;
        verts.extend_from_slice(&quad);
        indices.extend(tris.map(|i| start + i));
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ItemVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub tint: [f32; 3],
}

const GRID: usize = 16;
const DEPTH: f32 = 1.0 / 16.0;

const EDGE_UV_INSET_TEXELS: f32 = 1.0 / 64.0;

const SHADE_FRONT: f32 = 1.0;
const SHADE_BACK: f32 = 0.6;
const SHADE_SIDE: f32 = 0.8;

#[inline]
fn opaque(tile: Tile, tx: i32, ty: i32) -> bool {
    if tx < 0 || ty < 0 || tx >= GRID as i32 || ty >= GRID as i32 {
        return false;
    }
    let u = (tx as f32 + 0.5) / GRID as f32;
    let v_bottom_up = 1.0 - (ty as f32 + 0.5) / GRID as f32;
    tile_alpha_opaque(tile, u, v_bottom_up)
}

pub(crate) fn grip_point(tile: Tile, tool: bool) -> glam::Vec3 {
    static GRIPS: std::sync::LazyLock<Box<[[std::sync::OnceLock<glam::Vec3>; 2]]>> =
        std::sync::LazyLock::new(|| Tile::all().map(|_| Default::default()).collect());
    match GRIPS.get(tile.index()) {
        Some(grips) => *grips[usize::from(tool)].get_or_init(|| find_grip_point(tile, tool)),
        None => find_grip_point(tile, tool),
    }
}

fn find_grip_point(tile: Tile, tool: bool) -> glam::Vec3 {
    let texels: Vec<(i32, i32)> = (0..GRID as i32)
        .flat_map(|ty| (0..GRID as i32).map(move |tx| (tx, ty)))
        .filter(|&(tx, ty)| opaque(tile, tx, ty))
        .collect();
    let Some(lowest) = texels.iter().map(|&(_, ty)| ty).max() else {
        return glam::Vec3::ZERO;
    };
    let centre = |set: &mut dyn Iterator<Item = &(i32, i32)>| {
        let (mut sum, mut n) = (glam::Vec2::ZERO, 0.0);
        for &(tx, ty) in set {
            sum += glam::Vec2::new(px(tx) + 0.5 / GRID as f32, py(ty) - 0.5 / GRID as f32);
            n += 1.0;
        }
        sum / n
    };
    let end = centre(&mut texels.iter().filter(|&&(_, ty)| ty >= lowest - 1));
    let middle = centre(&mut texels.iter());
    let along = if tool { 0.35 } else { 0.12 };
    (end + (middle - end) * along).extend(0.0)
}

#[inline]
fn texel_uv_rect(tile: Tile, tx: i32, ty: i32) -> [f32; 4] {
    let [u0, v0, u1, v1] = tile_uv(tile);
    let du = (u1 - u0) / GRID as f32;
    let dv = (v1 - v0) / GRID as f32;
    let tu0 = u0 + du * tx as f32;
    let tv0 = v0 + dv * ty as f32;
    [tu0, tv0, tu0 + du, tv0 + dv]
}

#[inline]
fn px(tx: i32) -> f32 {
    tx as f32 / GRID as f32 - 0.5
}

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
    for &i in &[0usize, 1, 2, 0, 2, 3] {
        out.push(ItemVertex {
            pos: corners[i],
            uv: uvs[i],
            shade,
            tint,
        });
    }
}

pub(super) fn dye_item_verts(verts: &mut [ItemVertex], variant: petramond_world::item::VariantId) {
    let Some(t) = petramond_world::item::variant::tint(variant) else {
        return;
    };
    for v in verts.iter_mut() {
        v.tint = [v.tint[0] * t[0], v.tint[1] * t[1], v.tint[2] * t[2]];
        v.uv[1] += crate::atlas::DYE_V_OFFSET;
    }
}

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

pub(super) fn build_extruded_item_lit(
    tile: Tile,
    light: DynLight,
    env: LightEnv,
    out: &mut Vec<ItemVertex>,
) -> u32 {
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
/// base's pixel grid, visibly misaligning at 16 px. The
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
        dye_item_verts(out, variant);
        out.extend(geom.over.iter().map(lit));
    });
    out.len() as u32
}

struct CompositeGeometry {
    base: Vec<ItemVertex>,
    over: Vec<ItemVertex>,
}

thread_local! {
    static COMPOSITE_CACHE: std::cell::RefCell<
        rustc_hash::FxHashMap<(Tile, Vec<Tile>), CompositeGeometry>,
    > = std::cell::RefCell::new(rustc_hash::FxHashMap::default());
}

fn build_composited_geometry(tile: Tile, overlays: &[Tile]) -> CompositeGeometry {
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

fn build_extruded_item_geometry(tile: Tile) -> Vec<ItemVertex> {
    let mut verts = Vec::new();
    let out = &mut verts;

    let tint = [1.0, 1.0, 1.0];
    let zf = DEPTH * 0.5;
    let zb = -DEPTH * 0.5;
    // The faces span the tile edge to edge and the atlas has no gutters: a
    // corner uv exactly on the tile's boundary samples the NEIGHBOURING tile's
    // edge texels, a faint line along the slab's rim. A hair inside (far too
    // little to stretch a texel visibly, unlike a half-texel inset) keeps
    // every sample the face's own; the shaders interpolate uv at the centroid
    // so an antialiased edge fragment is not extrapolated back out.
    let [fu0, fv0, fu1, fv1] = {
        let [u0, v0, u1, v1] = tile_uv(tile);
        let (du, dv) = (
            (u1 - u0) / GRID as f32 * EDGE_UV_INSET_TEXELS,
            (v1 - v0) / GRID as f32 * EDGE_UV_INSET_TEXELS,
        );
        [u0 + du, v0 + dv, u1 - du, v1 - dv]
    };

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

    for ty in 0..GRID as i32 {
        for tx in 0..GRID as i32 {
            if !opaque(tile, tx, ty) {
                continue;
            }
            let [tu0, tv0, tu1, tv1] = texel_uv_rect(tile, tx, ty);
            let xl = px(tx);
            let xr = px(tx + 1);
            let yt = py(ty);
            let yb = py(ty + 1);
            let uc = [(tu0 + tu1) * 0.5, (tv0 + tv1) * 0.5];

            if !opaque(tile, tx - 1, ty) {
                push_quad(
                    out,
                    [[xl, yb, zb], [xl, yb, zf], [xl, yt, zf], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            if !opaque(tile, tx + 1, ty) {
                push_quad(
                    out,
                    [[xr, yb, zf], [xr, yb, zb], [xr, yt, zb], [xr, yt, zf]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
            if !opaque(tile, tx, ty - 1) {
                push_quad(
                    out,
                    [[xl, yt, zf], [xr, yt, zf], [xr, yt, zb], [xl, yt, zb]],
                    [uc, uc, uc, uc],
                    SHADE_SIDE,
                    tint,
                );
            }
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
        assert!(
            out.len() > 12,
            "expected front+back+walls, got {}",
            out.len()
        );
        for v in &out {
            assert!(v.pos[0] >= -0.5 - 1e-4 && v.pos[0] <= 0.5 + 1e-4);
            assert!(v.pos[1] >= -0.5 - 1e-4 && v.pos[1] <= 0.5 + 1e-4);
            assert!(v.pos[2].abs() <= DEPTH * 0.5 + 1e-4);
            assert!(
                v.shade == SHADE_FRONT || v.shade == SHADE_BACK || v.shade == SHADE_SIDE,
                "unexpected shade {}",
                v.shade
            );
        }
    }

    /// The composited slab is ONE extrusion on ONE pixel grid — the invariant
    /// required for aligned overlay texels. Every coordinate sits exactly on the 1/16 texel grid at
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
    fn full_faces_sample_strictly_inside_their_own_tile() {
        let mut out = Vec::new();
        build_extruded_item(Tile::named("poppy"), &mut out);
        let [u0, v0, u1, v1] = tile_uv(Tile::named("poppy"));
        let texel = (u1 - u0) / GRID as f32;
        for v in &out[..12] {
            let [u, w] = v.uv;
            assert!(u > u0 && u < u1 && w > v0 && w < v1, "uv on the tile edge");
            let edge = (u - u0).min(u1 - u).max((w - v0).min(v1 - w));
            assert!(edge < texel * 0.1, "inset large enough to stretch texels");
        }
    }

    #[test]
    fn rebuild_reuses_capacity() {
        let mut out = Vec::new();
        build_extruded_item(Tile::named("poppy"), &mut out);
        let cap = out.capacity();
        build_extruded_item(Tile::named("poppy"), &mut out);
        assert_eq!(
            out.capacity(),
            cap,
            "rebuild must reuse the buffer capacity"
        );
    }

    #[test]
    fn lit_extruded_item_folds_light_into_the_tint() {
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
        let mut out = Vec::new();
        build_extruded_item(Tile::named("stone"), &mut out);
        assert_eq!(out.len(), 12 + 4 * GRID * 6);
    }
}
