use super::state::Player;
use crate::atlas::{tile_alpha_bounds, tile_alpha_opaque, TileAlphaBounds};
use crate::block::{Block, RenderShape};
use crate::mathh::{IVec3, SelectionBoxes, SelectionShape, Vec3};
use crate::torch::{TorchPlacement, POLE_HALF, POLE_HEIGHT};
use crate::world::World;

/// Max block-interaction distance, measured from the eye.
pub const REACH: f32 = 4.0;
const EPS: f32 = 1.0e-5;

/// Server-side reach validation for a claimed block interaction: the CLOSEST
/// point of cell `block` within `REACH` (+1 slack for latency between the
/// claimed eye and the resolving tick) of `eye`. The one rule every
/// server-side block-reach check shares (look latch, `BreakFinished`).
pub(crate) fn block_within_reach(eye: Vec3, block: IVec3) -> bool {
    let lo = Vec3::new(block.x as f32, block.y as f32, block.z as f32);
    let closest = eye.clamp(lo, lo + Vec3::ONE);
    (closest - eye).length() <= REACH + 1.0
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct ShapeHit {
    t: f32,
    normal: Option<IVec3>,
}

impl ShapeHit {
    #[inline]
    fn distance(t: f32) -> Self {
        Self { t, normal: None }
    }

    #[inline]
    fn with_normal(t: f32, normal: IVec3) -> Self {
        Self {
            t,
            normal: Some(normal),
        }
    }
}

/// Result of a block raycast.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RaycastHit {
    /// The selected cell the ray entered.
    pub block: IVec3,
    /// Face normal pointing back toward the eye. `block + normal` is the empty
    /// cell to place into. Zero when the eye started inside the selected cell.
    pub normal: IVec3,
    pub outline: SelectionShape,
}

impl Player {
    /// Cast a ray from `eye` along (assumed-normalised) `dir` for the first
    /// selectable block within `REACH`, also returning the distance from `eye` to the
    /// hit (used to compare a block hit against a mob hit — the nearer wins, so looking
    /// at a mob interrupts block selection). Voxel DDA (Amanatides & Woo), with
    /// cross-model plants tested against their alpha-cutout billboards.
    pub(crate) fn raycast_with_dist(
        eye: Vec3,
        dir: Vec3,
        world: &World,
    ) -> Option<(RaycastHit, f32)> {
        let (mut hit, dist) = Self::raycast_blocks_core(
            eye,
            dir,
            &|x, y, z| Block::from_id(world.chunk_block(x, y, z)),
            // Inset/thin blocks (chest, torch) are tested against their real shape so
            // the ray only selects them where they actually are; the torch's tilt
            // comes from the world's per-chunk torch map.
            &|e, d, pos, block| precise_shape_hit(e, d, pos, block, world),
        )?;
        // A torch's outline traces its 3D pole, whose tilt depends on how it's
        // mounted — state that lives in the world's per-chunk torch map, not visible
        // to the block-only DDA core. Override the default full-cube outline here.
        let hit_block = Block::from_id(world.chunk_block(hit.block.x, hit.block.y, hit.block.z));
        if hit_block == Block::Torch {
            hit.outline = SelectionShape::Torch {
                origin: hit.block,
                transform: world.torch_placement(hit.block).model_transform(),
            };
        } else if matches!(hit_block.render_shape(), RenderShape::Model(_)) {
            // A bbmodel block outlines its WHOLE-MODEL bounding box (baked from geometry),
            // drawn as one box hugging the model's real extent across all its cells — not
            // a per-cell cube. (The DDA still TARGETS per cell, above.)
            if let Some((mn, mx)) = world.model_outline_box(hit.block) {
                hit.outline = SelectionShape::Box {
                    min: Vec3::from(mn),
                    max: Vec3::from(mx),
                };
            }
        } else if hit_block.render_shape() == RenderShape::Door {
            // A door outlines the actual slab where it is (facing + open state), so the
            // wireframe + break crack hug the panel rather than the row's default box.
            if let Some((mn, mx)) = world.selection_box_at(hit.block.x, hit.block.y, hit.block.z) {
                let base = Vec3::new(hit.block.x as f32, hit.block.y as f32, hit.block.z as f32);
                hit.outline = SelectionShape::Box {
                    min: base + Vec3::from(mn),
                    max: base + Vec3::from(mx),
                };
            }
        } else if hit_block.render_shape() == RenderShape::Stair {
            let (boxes, len) = crate::stair::world_boxes(
                hit.block,
                world.stair_boxes_at(hit.block.x, hit.block.y, hit.block.z),
            );
            hit.outline = SelectionShape::Boxes {
                boxes: SelectionBoxes { boxes, len },
            };
        } else if hit_block.render_shape() == RenderShape::Slab {
            let (boxes, len) = crate::slab::world_boxes(
                hit.block,
                world.slab_boxes_at(hit.block.x, hit.block.y, hit.block.z),
            );
            hit.outline = SelectionShape::Boxes {
                boxes: SelectionBoxes { boxes, len },
            };
        } else if hit_block.render_shape() == RenderShape::Pane {
            // A pane outlines its resolved post + arm runs, so the wireframe hugs
            // the connected shape the mesher drew, not the bare-post default.
            let (boxes, len) = crate::pane::world_boxes(hit.block, world.pane_boxes_at(hit.block));
            hit.outline = SelectionShape::Boxes {
                boxes: SelectionBoxes { boxes, len },
            };
        }
        Some((hit, dist))
    }

    /// Like [`raycast_with_dist`](Self::raycast_with_dist), but ANY water cell
    /// stops the ray too (as a full cube). Normal selection deliberately sees
    /// THROUGH water; a bucket POUR must target the water surface itself.
    /// Solids still stop the ray first. The caller inspects the hit cell's real
    /// block — the hit may be water or any normally selectable block, whichever
    /// the ray reaches first.
    pub(crate) fn raycast_including_water(
        eye: Vec3,
        dir: Vec3,
        world: &World,
    ) -> Option<(RaycastHit, f32)> {
        Self::raycast_water_stopping(eye, dir, world, |_| true)
    }

    /// Like [`raycast_including_water`](Self::raycast_including_water), but only
    /// water SOURCE cells stop the ray — FLOWING water stays transparent even to
    /// this ray. A bucket FILL only ever acts on a source, so a spread sheet or
    /// a thin film (both of which can render exactly like still water) must
    /// never shadow the source beneath or behind it: the ray reads through them
    /// to the source the player is actually aiming at.
    pub(crate) fn raycast_water_sources(
        eye: Vec3,
        dir: Vec3,
        world: &World,
    ) -> Option<(RaycastHit, f32)> {
        Self::raycast_water_stopping(eye, dir, world, |p| world.is_water_source_world(p))
    }

    /// Shared water-aware DDA: water cells satisfying `stops` read as full cubes
    /// (the ray hits them on cell entry), other water reads as air (transparent),
    /// and every non-water block behaves exactly as in normal selection.
    fn raycast_water_stopping<W: Fn(IVec3) -> bool>(
        eye: Vec3,
        dir: Vec3,
        world: &World,
        stops: W,
    ) -> Option<(RaycastHit, f32)> {
        Self::raycast_blocks_core(
            eye,
            dir,
            &|x, y, z| {
                let b = Block::from_id(world.chunk_block(x, y, z));
                if b == Block::Water {
                    if stops(IVec3::new(x, y, z)) {
                        Block::Stone
                    } else {
                        Block::Air
                    }
                } else {
                    b
                }
            },
            &|e, d, pos, block| precise_shape_hit(e, d, pos, block, world),
        )
    }

    #[cfg(test)]
    pub(super) fn raycast_core<F: Fn(i32, i32, i32) -> bool>(
        eye: Vec3,
        dir: Vec3,
        solid: &F,
    ) -> Option<RaycastHit> {
        // The stub world is only full cubes, so the precise-shape closure is never
        // consulted (full cubes hit on cell entry).
        Self::raycast_blocks_core(
            eye,
            dir,
            &|x, y, z| {
                if solid(x, y, z) {
                    Block::Stone
                } else {
                    Block::Air
                }
            },
            &|_, _, _, _| None,
        )
        .map(|(hit, _)| hit)
    }

    /// The DDA core, returning the hit and its distance from `eye` (the entry
    /// parameter — `t_enter` for a full cube, the precise crossing `t` for a
    /// custom-shaped block / cross-plant).
    pub(super) fn raycast_blocks_core<F, S>(
        eye: Vec3,
        dir: Vec3,
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
            let t_exit = next_boundary_t(t_max);
            // The "solid body" selection branch: genuinely solid blocks plus the
            // torch — not solid, but still selectable by its thin pole shape (the
            // cross-plant case is handled separately below, like its render shape).
            if block.is_solid() || block.render_shape() == RenderShape::Torch {
                // A full cube fills its cell, so it stops the ray on entry. A
                // custom-shaped block (the inset chest, the thin/tilted torch pole)
                // only registers when the ray actually crosses its shape — otherwise
                // the ray sees past the empty parts of its cell.
                if block.visual_aabb().is_none()
                    && block != Block::Torch
                    && block.render_shape() != RenderShape::Stair
                    && block.render_shape() != RenderShape::Slab
                {
                    return Some((hit(pos, entry_normal, block), t_enter));
                }
                if let Some(shape) = shape_hit(eye, dir, pos, block) {
                    let t = shape.t;
                    if t + EPS >= t_enter && t <= t_exit + EPS && t <= REACH {
                        return Some((hit(pos, shape.normal.unwrap_or(entry_normal), block), t));
                    }
                }
            } else if block.render_shape() == RenderShape::Cross {
                if let Some(t) = intersect_cross_plant(eye, dir, pos, block) {
                    if t + EPS >= t_enter && t <= t_exit + EPS && t <= REACH {
                        return Some((hit(pos, entry_normal, block), t));
                    }
                }
            }

            // Advance across the nearest voxel boundary.
            let (axis, t_exit) = if t_max.x <= t_max.y && t_max.x <= t_max.z {
                (0, t_max.x)
            } else if t_max.y <= t_max.z {
                (1, t_max.y)
            } else {
                (2, t_max.z)
            };
            if t_exit > REACH {
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
}

fn hit(block_pos: IVec3, normal: IVec3, block: Block) -> RaycastHit {
    RaycastHit {
        block: block_pos,
        normal,
        outline: outline_shape(block_pos, block),
    }
}

fn outline_shape(block_pos: IVec3, block: Block) -> SelectionShape {
    // Non-full-cube blocks (the chest) outline their inset visual box.
    if let Some((mn, mx)) = block.visual_aabb() {
        let base = Vec3::new(block_pos.x as f32, block_pos.y as f32, block_pos.z as f32);
        return SelectionShape::Box {
            min: base + Vec3::new(mn[0], mn[1], mn[2]),
            max: base + Vec3::new(mx[0], mx[1], mx[2]),
        };
    }
    if block.render_shape() != RenderShape::Cross {
        return SelectionShape::full_block(block_pos);
    }

    let Some(bounds) = tile_alpha_bounds(block.tiles()[0]) else {
        return SelectionShape::full_block(block_pos);
    };

    if should_outline_as_full_block(bounds) {
        return SelectionShape::full_block(block_pos);
    }

    SelectionShape::Cross {
        origin: block_pos,
        u_min: bounds.u_min,
        u_max: bounds.u_max,
        v_min: bounds.v_min,
        v_max: bounds.v_max,
    }
}

fn should_outline_as_full_block(bounds: TileAlphaBounds) -> bool {
    let width = bounds.u_max - bounds.u_min;
    let height = bounds.v_max - bounds.v_min;
    width >= 0.875 && height >= 0.875
}

/// Distance along the ray to the first crossing of a block's PRECISE shape — the
/// inset chest body, or the torch pole — or `None` if the ray misses it. Full cubes
/// never reach here (they stop the ray on cell entry); this is what lets selection
/// ignore the empty parts of an inset/thin block's cell.
fn precise_shape_hit(
    eye: Vec3,
    dir: Vec3,
    pos: IVec3,
    block: Block,
    world: &World,
) -> Option<ShapeHit> {
    if block == Block::Torch {
        return ray_vs_torch(eye, dir, pos, world.torch_placement(pos));
    }
    // A bbmodel block is picked PIXEL-PERFECT: the ray is tested against the actual
    // posed cubes of the whole model (in footprint space) with the entry face alpha-
    // tested, so aiming through the gap between the legs / under the top / at a cut-out
    // texel misses instead of selecting the block. The DDA's per-cell `t <= t_exit`
    // window then attributes the crossing to whichever cell the surface falls in.
    if let RenderShape::Model(kind) = block.render_shape() {
        let off = world.model_offset_at(pos.x, pos.y, pos.z);
        let facing = world.model_facing_at(pos.x, pos.y, pos.z);
        let base = crate::block_model::base_from_cell(pos, kind, off, facing);
        let inv = crate::block_model::placement_transform(base, kind, facing).inverse();
        return crate::block_model::ray_vs_model(
            inv.transform_point3(eye),
            inv.transform_vector3(dir),
            kind,
        )
        .map(ShapeHit::distance);
    }
    // A door's thin slab depends on its facing + open state (the chunk door map), so
    // test the position-aware box rather than the block row's position-less default.
    if block.render_shape() == RenderShape::Door {
        let (mn, mx) = world.selection_box_at(pos.x, pos.y, pos.z)?;
        let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        return ray_vs_aabb_hit(eye, dir, base + Vec3::from(mn), base + Vec3::from(mx));
    }
    if block.render_shape() == RenderShape::Stair {
        let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        return world
            .stair_boxes_at(pos.x, pos.y, pos.z)
            .iter()
            .filter_map(|b| {
                ray_vs_aabb_hit(
                    eye,
                    dir,
                    base + Vec3::new(b.min[0], b.min[1], b.min[2]),
                    base + Vec3::new(b.max[0], b.max[1], b.max[2]),
                )
            })
            .min_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }
    if block.render_shape() == RenderShape::Slab {
        let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        return world
            .slab_boxes_at(pos.x, pos.y, pos.z)
            .iter()
            .filter_map(|b| {
                ray_vs_aabb_hit(
                    eye,
                    dir,
                    base + Vec3::new(b.min[0], b.min[1], b.min[2]),
                    base + Vec3::new(b.max[0], b.max[1], b.max[2]),
                )
            })
            .min_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }
    // A pane is picked against its resolved post + arm runs (neighbour-derived,
    // like the stair's corner boxes), so the ray connects where the glass is.
    if block.render_shape() == RenderShape::Pane {
        let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        return world
            .pane_boxes_at(pos)
            .iter()
            .filter_map(|b| {
                ray_vs_aabb_hit(
                    eye,
                    dir,
                    base + Vec3::new(b.min[0], b.min[1], b.min[2]),
                    base + Vec3::new(b.max[0], b.max[1], b.max[2]),
                )
            })
            .min_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }
    // Any other custom-shaped solid (the chest) tests its inset visual box.
    let (mn, mx) = block.visual_aabb()?;
    let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
    ray_vs_aabb_hit(eye, dir, base + Vec3::from(mn), base + Vec3::from(mx))
}

/// First-crossing distance of the ray through the torch's pole box. The pole is a
/// thin, possibly-tilted box, so transform the ray into the torch's local model
/// space (the inverse of its placement transform — a rigid rotate+translate, so
/// distances along the ray are preserved) and test the upright local box.
fn ray_vs_torch(eye: Vec3, dir: Vec3, pos: IVec3, placement: TorchPlacement) -> Option<ShapeHit> {
    let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
    let inv = placement.model_transform().inverse();
    let ol = inv.transform_point3(eye - base);
    let dl = inv.transform_vector3(dir);
    ray_vs_aabb(
        ol,
        dl,
        Vec3::new(-POLE_HALF, 0.0, -POLE_HALF),
        Vec3::new(POLE_HALF, POLE_HEIGHT, POLE_HALF),
    )
    .map(ShapeHit::distance)
}

/// Ray vs axis-aligned box (slab method): the entry distance `t >= 0`, or `None`
/// when the ray misses the box or it lies entirely behind the eye. Shared with mob
/// targeting (a mob is an AABB) — that's why it's crate-visible.
pub(crate) fn ray_vs_aabb(eye: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    ray_vs_aabb_hit(eye, dir, min, max).map(|hit| hit.t)
}

/// Ray vs axis-aligned box with the crossed face normal. The normal points back
/// toward the ray origin, matching [`RaycastHit::normal`].
pub(super) fn ray_vs_aabb_hit(eye: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<ShapeHit> {
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
            // Ray parallel to this slab: miss unless the origin is within it.
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

fn intersect_cross_plant(eye: Vec3, dir: Vec3, block_pos: IVec3, block: Block) -> Option<f32> {
    let tile = block.tiles()[0];
    let x = block_pos.x as f32;
    let y = block_pos.y as f32;
    let z = block_pos.z as f32;
    let mut best = f32::INFINITY;

    // Plane 1: (x,z) -> (x+1,z+1).
    let denom = dir.x - dir.z;
    if denom.abs() > EPS {
        let numer = -((eye.x - x) - (eye.z - z));
        let t = numer / denom;
        if t >= -EPS {
            let p = eye + dir * t;
            let u = p.x - x;
            let v = p.y - y;
            if in_unit(u) && in_unit(v) && tile_alpha_opaque(tile, u, v) {
                best = best.min(t.max(0.0));
            }
        }
    }

    // Plane 2: (x,z+1) -> (x+1,z).
    let denom = dir.x + dir.z;
    if denom.abs() > EPS {
        let numer = -((eye.x - x) + (eye.z - (z + 1.0)));
        let t = numer / denom;
        if t >= -EPS {
            let p = eye + dir * t;
            let u = p.x - x;
            let v = p.y - y;
            if in_unit(u) && in_unit(v) && tile_alpha_opaque(tile, u, v) {
                best = best.min(t.max(0.0));
            }
        }
    }

    best.is_finite().then_some(best)
}

#[inline]
fn in_unit(v: f32) -> bool {
    (-EPS..=1.0 + EPS).contains(&v)
}

#[inline]
fn next_boundary_t(t_max: Vec3) -> f32 {
    t_max.x.min(t_max.y).min(t_max.z)
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

/// 1/|v|, or +∞ when v is zero (that axis is never crossed).
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
fn boundary_t(p: f32, d: f32) -> f32 {
    if d == 0.0 {
        return f32::INFINITY;
    }
    let cell = p.floor();
    if d > 0.0 {
        (cell + 1.0 - p) / d
    } else {
        (p - cell) / -d
    }
}
