use crate::block::{Block, MeshEmitter, PlantPlanes};
use crate::item::UseRay;
use crate::selection::{SelectionBoxes, SelectionShape, MAX_SELECTION_BOXES};
use crate::tile_alpha::{tile_alpha_bounds, TileAlphaBounds};
use crate::torch::{TorchPlacement, POLE_HALF, POLE_HEIGHT};
use crate::world::WorldData;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;

pub const REACH: f32 = 4.0;
const EPS: f32 = 1.0e-5;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeHit {
    t: f32,
    normal: Option<IVec3>,
}

impl ShapeHit {
    #[inline]
    pub fn distance(t: f32) -> Self {
        Self { t, normal: None }
    }

    #[inline]
    pub fn with_normal(t: f32, normal: IVec3) -> Self {
        Self {
            t,
            normal: Some(normal),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RayFilter {
    Selectable,
    Collidable,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RaycastHit {
    pub block: IVec3,
    pub normal: IVec3,
    pub spot: Vec3,
    pub outline: SelectionShape,
}

pub fn with_dist(eye: WorldPos, dir: Vec3, world: &WorldData) -> Option<(RaycastHit, f32)> {
    let (mut hit, dist) = blocks_core(
        eye,
        dir,
        REACH,
        &|x, y, z| Block::from_id(world.chunk_block(x, y, z)),
        &|e, d, pos, block| precise_shape_hit(e, d, pos, block, world),
    )?;
    let hit_block = Block::from_id(world.chunk_block(hit.block.x, hit.block.y, hit.block.z));
    if is_pole(hit_block) {
        hit.outline = SelectionShape::Posed {
            origin: hit.block,
            transform: world.torch_placement(hit.block).model_transform(),
            min: Vec3::new(-POLE_HALF, 0.0, -POLE_HALF),
            max: Vec3::new(POLE_HALF, POLE_HEIGHT, POLE_HALF),
        };
    } else if hit_block.model_kind().is_some() {
        if let Some((base, mn, mx)) = world.model_outline_box(hit.block) {
            hit.outline = SelectionShape::Box {
                origin: base,
                min: Vec3::from(mn),
                max: Vec3::from(mx),
            };
        }
    } else if hit_block.picks_by_boxes() {
        let mut targets = Vec::new();
        world.target_boxes_at(hit.block.x, hit.block.y, hit.block.z, &mut targets);
        let boxes: Vec<crate::block::Aabb> = targets
            .iter()
            .filter_map(|b| b.bounds().clipped_to_cell())
            .collect();
        let boxes = &boxes[..];
        if boxes.is_empty() {
        } else if boxes.len() <= MAX_SELECTION_BOXES {
            let (boxes, len) = crate::connect::local_boxes(boxes);
            hit.outline = SelectionShape::Boxes {
                origin: hit.block,
                boxes: SelectionBoxes { boxes, len },
            };
        } else {
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            for b in boxes {
                for a in 0..3 {
                    mn[a] = mn[a].min(b.min[a]);
                    mx[a] = mx[a].max(b.max[a]);
                }
            }
            hit.outline = SelectionShape::Box {
                origin: hit.block,
                min: Vec3::from(mn),
                max: Vec3::from(mx),
            };
        }
    } else if let Some((mn, mx)) = world.selection_box_at(hit.block.x, hit.block.y, hit.block.z) {
        hit.outline = SelectionShape::Box {
            origin: hit.block,
            min: Vec3::from(mn),
            max: Vec3::from(mx),
        };
    }
    Some((hit, dist))
}

pub fn filtered(
    eye: WorldPos,
    dir: Vec3,
    max: f32,
    filter: RayFilter,
    world: &WorldData,
) -> Option<(RaycastHit, f32)> {
    let shape_hit = |e, d, pos, block| precise_shape_hit(e, d, pos, block, world);
    match filter {
        RayFilter::Selectable => blocks_core(
            eye,
            dir,
            max,
            &|x, y, z| Block::from_id(world.chunk_block(x, y, z)),
            &shape_hit,
        ),
        RayFilter::Collidable => blocks_core(
            eye,
            dir,
            max,
            &|x, y, z| {
                let block = Block::from_id(world.chunk_block(x, y, z));
                if block == Block::Air || world.collision_boxes_at(x, y, z).is_empty() {
                    Block::Air
                } else {
                    block
                }
            },
            &shape_hit,
        ),
    }
}

pub fn use_ray(
    eye: WorldPos,
    dir: Vec3,
    world: &WorldData,
    ray: UseRay,
) -> Option<(RaycastHit, f32)> {
    fluid_stopping(eye, dir, world, |_, fluid| ray.stops_at(fluid))
}

pub fn including_any_fluid(
    eye: WorldPos,
    dir: Vec3,
    world: &WorldData,
) -> Option<(RaycastHit, f32)> {
    fluid_stopping(eye, dir, world, |_, _| true)
}

pub fn fluid_sources(
    eye: WorldPos,
    dir: Vec3,
    world: &WorldData,
    scoops: impl Fn(Block) -> bool,
) -> Option<(RaycastHit, f32)> {
    fluid_stopping(eye, dir, world, |p, fluid| {
        scoops(fluid) && world.is_fluid_source_world(p, fluid)
    })
}

fn fluid_stopping<W: Fn(IVec3, Block) -> bool>(
    eye: WorldPos,
    dir: Vec3,
    world: &WorldData,
    stops: W,
) -> Option<(RaycastHit, f32)> {
    blocks_core(
        eye,
        dir,
        REACH,
        &|x, y, z| {
            let b = Block::from_id(world.chunk_block(x, y, z));
            match b.fluid() {
                Some(fluid) if fluid == b => {
                    if stops(IVec3::new(x, y, z), fluid) {
                        Block::Stone
                    } else {
                        Block::Air
                    }
                }
                _ => b,
            }
        },
        &|e, d, pos, block| precise_shape_hit(e, d, pos, block, world),
    )
}

pub fn blocks_core<F, S>(
    eye: WorldPos,
    dir: Vec3,
    max: f32,
    block_at: &F,
    shape_hit: &S,
) -> Option<(RaycastHit, f32)>
where
    F: Fn(i32, i32, i32) -> Block,
    S: Fn(Vec3, Vec3, IVec3, Block) -> Option<ShapeHit>,
{
    if dir.length_squared() <= f32::EPSILON {
        return None;
    }

    let mut ix = eye.x.floor() as i32;
    let mut iy = eye.y.floor() as i32;
    let mut iz = eye.z.floor() as i32;

    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    let t_delta = Vec3::new(inv_abs(dir.x), inv_abs(dir.y), inv_abs(dir.z));
    let mut t_max = Vec3::new(
        boundary_t(eye.x, dir.x),
        boundary_t(eye.y, dir.y),
        boundary_t(eye.z, dir.z),
    );
    let mut t_enter = 0.0;
    let mut entry_normal = IVec3::ZERO;

    loop {
        let pos = IVec3::new(ix, iy, iz);
        let block = block_at(ix, iy, iz);
        // Solid-body branch covers solid blocks, shapes needing a precise ray test (box-set,
        // torch, wall panel, custom-shape chair), and non-colliding shapes with an aabb
        // (snow layer, no-collision model). Both come from shape facets, so a modded
        // shape needs no engine change here. Cross-plant is handled separately below.
        // Read both once, the table lookups run per stepped cell.
        let precise_only = block.precise_pick();
        let shape_box = block.visual_aabb();
        if block.is_solid() || precise_only || shape_box.is_some() {
            let local_eye = eye - WorldPos::block_min(pos);
            if shape_box.is_none() && !precise_only {
                return Some((
                    hit(pos, entry_normal, block, local_eye + dir * t_enter),
                    t_enter,
                ));
            }
            if let Some(shape) = shape_hit(local_eye, dir, pos, block) {
                let t = shape.t;
                if t <= max && hit_in_cell(local_eye, dir, t) {
                    return Some((
                        hit(
                            pos,
                            shape.normal.unwrap_or(entry_normal),
                            block,
                            local_eye + dir * t,
                        ),
                        t,
                    ));
                }
            }
        } else if let Some((mn, mx)) = plant_selection_aabb(block) {
            let local_eye = eye - WorldPos::block_min(pos);
            if let Some(shape) = ray_vs_aabb_hit(local_eye, dir, mn, mx) {
                let t = shape.t;
                if t <= max && hit_in_cell(local_eye, dir, t) {
                    return Some((
                        hit(
                            pos,
                            shape.normal.unwrap_or(entry_normal),
                            block,
                            local_eye + dir * t,
                        ),
                        t,
                    ));
                }
            }
        }

        let (axis, t_exit) = if t_max.x <= t_max.y && t_max.x <= t_max.z {
            (0, t_max.x)
        } else if t_max.y <= t_max.z {
            (1, t_max.y)
        } else {
            (2, t_max.z)
        };
        if t_exit > max {
            return None;
        }
        let mut normal = IVec3::ZERO;
        match axis {
            0 => {
                ix += step.x;
                t_max.x += t_delta.x;
                normal.x = -step.x;
            }
            1 => {
                iy += step.y;
                t_max.y += t_delta.y;
                normal.y = -step.y;
            }
            _ => {
                iz += step.z;
                t_max.z += t_delta.z;
                normal.z = -step.z;
            }
        }
        t_enter = t_exit;
        entry_normal = normal;
    }
}

fn hit(block_pos: IVec3, normal: IVec3, block: Block, spot: Vec3) -> RaycastHit {
    RaycastHit {
        block: block_pos,
        normal,
        spot: spot.clamp(Vec3::ZERO, Vec3::ONE),
        outline: outline_shape(block_pos, block),
    }
}

fn outline_shape(block_pos: IVec3, block: Block) -> SelectionShape {
    let local = block
        .visual_aabb()
        .map(|(mn, mx)| (Vec3::from(mn), Vec3::from(mx)))
        .or_else(|| plant_selection_aabb(block));
    match local {
        Some((min, max)) => SelectionShape::Box {
            origin: block_pos,
            min,
            max,
        },
        None => SelectionShape::full_block(block_pos),
    }
}

/// Selection box for plant blocks (`Cross`, `Crop`), local to the cell. Targeting and
/// outline share this AABB, so aiming inside the outline hits. Comes from the
/// art's opaque bounds, trimmed to height and width, so a ray can pass over
/// short grass to whatever's behind or below it. Dense art or art with no
/// bounds falls back to the full cell. `None` for everything else. Solids,
/// models and torches keep their own precise ray tests, which is why you can aim
/// past a chest or through a bbmodel's empty parts.
fn plant_selection_aabb(block: Block) -> Option<(Vec3, Vec3)> {
    let MeshEmitter::Plant(layout) = block.shape_kind_def().mesh_emitter else {
        return None;
    };
    let trimmed = tile_alpha_bounds(block.tiles()[0]).filter(|b| !should_outline_as_full_block(*b));
    let Some(b) = trimmed else {
        return Some((Vec3::ZERO, Vec3::ONE));
    };
    Some(match layout {
        PlantPlanes::Crop => {
            let inset = crate::block::CROP_PLANE_INSET;
            let drop = crate::block::CROP_PLANE_DROP;
            (
                Vec3::new(inset, b.v_min - drop, inset),
                Vec3::new(1.0 - inset, b.v_max - drop, 1.0 - inset),
            )
        }
        PlantPlanes::Cross => {
            let lo = b.u_min.min(1.0 - b.u_max);
            let hi = b.u_max.max(1.0 - b.u_min);
            (Vec3::new(lo, b.v_min, lo), Vec3::new(hi, b.v_max, hi))
        }
    })
}

fn should_outline_as_full_block(bounds: TileAlphaBounds) -> bool {
    let width = bounds.u_max - bounds.u_min;
    let height = bounds.v_max - bounds.v_min;
    width >= 0.875 && height >= 0.875
}

fn precise_shape_hit(
    eye: Vec3,
    dir: Vec3,
    pos: IVec3,
    block: Block,
    world: &WorldData,
) -> Option<ShapeHit> {
    if is_pole(block) {
        return ray_vs_torch(eye, dir, world.torch_placement(pos));
    }
    // A bbmodel block is picked PIXEL-PERFECT: the ray is tested against the actual
    // posed cubes of the whole model (in footprint space) with the entry face alpha-
    // tested, so aiming through the gap between the legs / under the top / at a cut-out
    // texel misses instead of selecting the block. The DDA's per-cell `t <= t_exit`
    // window then attributes the crossing to whichever cell the surface falls in.
    if let Some(kind) = block.model_kind() {
        let off = world.model_offset_at(pos.x, pos.y, pos.z);
        let facing = world.model_facing_at(pos.x, pos.y, pos.z);
        let base = crate::block_model::base_from_cell(pos, kind, off, facing);
        let inv = crate::block_model::placement_transform(kind, facing).inverse();
        let cell = (pos - base).as_vec3();
        // Restrict crossings to THIS cell (in footprint space): a `fit: native`
        // model's overhang lives outside every footprint cell, so a global
        // first crossing on it would veto the in-cell geometry behind it and
        // let the ray select the block beyond the machine. The placement
        // transform is a 90° yaw + translation, so the cell stays a box.
        let ca = inv.transform_point3(cell);
        let cb = inv.transform_point3(cell + Vec3::ONE);
        return crate::block_model::ray_vs_model_within(
            inv.transform_point3(eye + cell),
            inv.transform_vector3(dir),
            kind,
            ca.min(cb),
            ca.max(cb),
        )
        .map(ShapeHit::distance);
    }
    // Box-set families (stairs, slabs, panes, fences, WASM shape bakes, static
    // box sets) get picked against their resolved target boxes, posed or not.
    // Same facet as collision, so aimed boxes match collision boxes and you
    // can't hit through a gap. A cache miss on a WASM bake falls back to the
    // row's static boxes, so custom shapes stay aimable.
    if block.picks_by_boxes() {
        let mut targets = Vec::new();
        world.target_boxes_at(pos.x, pos.y, pos.z, &mut targets);
        return targets
            .iter()
            .filter_map(|b| match b.pose {
                None => ray_vs_aabb_hit(eye, dir, Vec3::from(b.aabb.min), Vec3::from(b.aabb.max)),
                Some(pose) => pose
                    .ray_hit(eye, dir, b.aabb.min, b.aabb.max)
                    .map(|(t, n)| ShapeHit::with_normal(t, dominant_axis(n))),
            })
            .min_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }
    let (mn, mx) = world.selection_box_at(pos.x, pos.y, pos.z)?;
    ray_vs_aabb_hit(eye, dir, Vec3::from(mn), Vec3::from(mx))
}

fn dominant_axis(n: Vec3) -> IVec3 {
    let a = n.abs();
    if a.y >= a.x && a.y >= a.z {
        IVec3::new(0, if n.y >= 0.0 { 1 } else { -1 }, 0)
    } else if a.x >= a.z {
        IVec3::new(if n.x >= 0.0 { 1 } else { -1 }, 0, 0)
    } else {
        IVec3::new(0, 0, if n.z >= 0.0 { 1 } else { -1 })
    }
}

fn is_pole(block: Block) -> bool {
    block.shape_kind_def().mesh_emitter == MeshEmitter::Pole
}

fn ray_vs_torch(eye: Vec3, dir: Vec3, placement: TorchPlacement) -> Option<ShapeHit> {
    let inv = placement.model_transform().inverse();
    let ol = inv.transform_point3(eye);
    let dl = inv.transform_vector3(dir);
    ray_vs_aabb(
        ol,
        dl,
        Vec3::new(-POLE_HALF, 0.0, -POLE_HALF),
        Vec3::new(POLE_HALF, POLE_HEIGHT, POLE_HALF),
    )
    .map(ShapeHit::distance)
}

pub fn ray_vs_aabb(eye: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    ray_vs_aabb_hit(eye, dir, min, max).map(|hit| hit.t)
}

pub fn ray_vs_aabb_hit(eye: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<ShapeHit> {
    let (e, d, lo, hi) = (
        eye.to_array(),
        dir.to_array(),
        min.to_array(),
        max.to_array(),
    );
    let mut t_near = f32::NEG_INFINITY;
    let mut t_far = f32::INFINITY;
    let mut normal = IVec3::ZERO;
    for i in 0..3 {
        if d[i].abs() < EPS {
            if e[i] < lo[i] - EPS || e[i] > hi[i] + EPS {
                return None;
            }
        } else {
            let inv = 1.0 / d[i];
            let mut t1 = (lo[i] - e[i]) * inv;
            let mut t2 = (hi[i] - e[i]) * inv;
            let mut n1 = axis_normal(i, -1);
            let mut n2 = axis_normal(i, 1);
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
                std::mem::swap(&mut n1, &mut n2);
            }
            if t1 > t_near {
                normal = n1;
            }
            t_near = t_near.max(t1);
            t_far = t_far.min(t2);
            if t_near > t_far {
                return None;
            }
        }
    }
    if t_far < 0.0 {
        return None;
    }
    if t_near < 0.0 {
        Some(ShapeHit::with_normal(0.0, IVec3::ZERO))
    } else {
        Some(ShapeHit::with_normal(t_near, normal))
    }
}

#[inline]
fn axis_normal(axis: usize, sign: i32) -> IVec3 {
    match axis {
        0 => IVec3::new(sign, 0, 0),
        1 => IVec3::new(0, sign, 0),
        _ => IVec3::new(0, 0, sign),
    }
}

#[inline]
fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

#[inline]
fn inv_abs(v: f32) -> f32 {
    if v == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / v).abs()
    }
}

/// Distance along the ray from `p` to the first voxel boundary in direction `d`.
#[inline]
fn boundary_t(p: f64, d: f32) -> f32 {
    if d == 0.0 {
        return f32::INFINITY;
    }
    let frac = (p - p.floor()) as f32;
    if d > 0.0 {
        (1.0 - frac) / d
    } else {
        frac / -d
    }
}

/// Whether a precise pick's hit point belongs to its cell: inside the cell's
/// box, or a seam's width outside it. `eye` is relative to the cell.
/// Comparing the pick's `t` against the DDA's entry/exit times instead
/// rejects a face lying EXACTLY on a cell seam: the boundary times accumulate
/// float error cell by cell, so the face lands in the crack between one
/// cell's exit and the next cell's entry and both cells reject it — the ray
/// sails straight through solid geometry (the forging furnace's hood face sits
/// exactly on its footprint's z-seam). The hit point comes from the pick's own
/// `t`, and an inflated box accepts a seam face in the first tested cell that
/// touches it — which is never a phantom hit, only ever the face's own cell or
/// its seam neighbour.
#[inline]
fn hit_in_cell(eye: Vec3, dir: Vec3, t: f32) -> bool {
    const SEAM: f32 = 1e-3;
    let p = eye + dir * t;
    p.to_array()
        .iter()
        .all(|&v| (-SEAM..=1.0 + SEAM).contains(&v))
}
