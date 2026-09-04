//! The flight sweep: the first thing along a flying item's motion —
//! collidable terrain, a player body, or a mob — with the mobs bucketed by
//! section once per tick, so each sweep asks only the ones its segment can
//! reach instead of every instance in the world.

use std::collections::HashMap;

use crate::mob::EntityRef;
use crate::player::RayFilter;
use crate::world::World;
use petramond_math::math::{IVec3, Vec3};

use super::step::StepCtx;
use super::ImpactTarget;

/// The live mobs, bucketed by the 16³ section holding their position.
pub(super) struct SweepBodies {
    /// Instance indices per section, in instance order.
    buckets: HashMap<IVec3, Vec<u32>>,
    /// The largest extent any bucketed body reaches from its position, in
    /// blocks: the margin a segment's section range grows by so a body
    /// standing in a neighbouring section is still a candidate.
    reach: f32,
}

impl SweepBodies {
    /// Bucket every mob that can be struck. Gathered once per tick, only
    /// while something is in flight.
    pub(super) fn gather(world: &World) -> Self {
        let mut buckets: HashMap<IVec3, Vec<u32>> = HashMap::new();
        let mut reach: f32 = 0.0;
        for (i, m) in world.mobs().instances().iter().enumerate() {
            if m.is_dead() {
                continue;
            }
            let size = crate::mob::def(m.kind).size;
            reach = reach
                .max(size.half_length.unwrap_or(0.0))
                .max(size.half_width)
                .max(size.height);
            buckets.entry(section_of(m.pos)).or_default().push(i as u32);
        }
        SweepBodies { buckets, reach }
    }

    /// Instance indices of the mobs whose section the segment `from`→`to`
    /// crosses, grown by the bodies' reach, in instance order (the same
    /// candidates in the same order whatever the segment, so the nearest-hit
    /// rule stays deterministic).
    fn near(&self, from: Vec3, to: Vec3) -> Vec<u32> {
        if self.buckets.is_empty() {
            return Vec::new();
        }
        let margin = Vec3::splat(self.reach);
        let lo = section_of(from.min(to) - margin);
        let hi = section_of(from.max(to) + margin);
        let mut out = Vec::new();
        for sx in lo.x..=hi.x {
            for sy in lo.y..=hi.y {
                for sz in lo.z..=hi.z {
                    if let Some(bucket) = self.buckets.get(&IVec3::new(sx, sy, sz)) {
                        out.extend_from_slice(bucket);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// The section holding world position `p` (a plain floor-divide per axis;
/// the vertical range is irrelevant to a bucket key).
fn section_of(p: Vec3) -> IVec3 {
    IVec3::new(
        (p.x.floor() as i32) >> 4,
        (p.y.floor() as i32) >> 4,
        (p.z.floor() as i32) >> 4,
    )
}

/// The first thing a flight from `from` along `motion` strikes: the nearest
/// of the collidable terrain (`RayFilter::Collidable` — a plant or a snow
/// layer is flown through) and the live bodies (every anchor with a body,
/// every bucketed mob), the launcher excepted while `spared` names it. A
/// tie goes to the terrain: a body pressed against a wall is struck
/// through the wall on the next tick, never the wall through the body.
pub(super) fn sweep(
    ctx: &StepCtx,
    from: Vec3,
    motion: Vec3,
    spared: Option<EntityRef>,
) -> Option<(ImpactTarget, Vec3)> {
    let length = motion.length();
    if length <= 1e-6 {
        return None;
    }
    let dir = motion / length;
    let world = ctx.world;
    let terrain =
        crate::player::Player::raycast_filtered(from, dir, length, RayFilter::Collidable, world)
            .map(|(hit, distance)| {
                (
                    distance,
                    ImpactTarget::Block {
                        cell: hit.block,
                        face: hit.normal,
                    },
                )
            });
    let limit = terrain.map_or(length, |(d, _)| d);
    let mut body: Option<(f32, ImpactTarget)> = None;
    let mut consider = |distance: f32, target: ImpactTarget| {
        if distance <= limit && body.is_none_or(|(best, _)| distance < best) {
            body = Some((distance, target));
        }
    };
    for anchor in ctx.anchors {
        if spared == Some(EntityRef::Player(anchor.id)) {
            continue;
        }
        let Some(b) = anchor.body else {
            continue;
        };
        let (lo, hi) = b.aabb();
        if let Some(t) = crate::player::ray_vs_aabb(from, dir, lo, hi) {
            consider(t, ImpactTarget::Player(anchor.id));
        }
    }
    if let Some(bodies) = &ctx.bodies {
        // A sphere around the swept segment rejects the rest of the
        // candidates before their body boxes are built: only a body that
        // could reach the segment is tested — the flight's half-length plus
        // the body's own extent.
        let mid = from + dir * (limit * 0.5);
        let near = |pos: Vec3, size: crate::mob::MobSize| {
            let reach =
                limit * 0.5 + size.half_length.unwrap_or(0.0).max(size.half_width) + size.height;
            (pos - mid).length_squared() <= reach * reach
        };
        let instances = world.mobs().instances();
        let mobs = bodies
            .near(from, from + motion)
            .into_iter()
            .map(|i| &instances[i as usize])
            .filter(|m| spared != Some(EntityRef::Mob(m.id())))
            .map(|m| (m.id(), m.pos, m.yaw, crate::mob::def(m.kind).size))
            .filter(|(_, pos, _, size)| near(*pos, *size));
        if let Some((id, t)) = crate::mob::closest_body_ray_hit(from, dir, limit, mobs) {
            consider(t, ImpactTarget::Mob(id));
        }
    }
    let (distance, target) = match (body, terrain) {
        (Some(b), Some(t)) if b.0 < t.0 => b,
        (Some(b), None) => b,
        (_, Some(t)) => t,
        (None, None) => return None,
    };
    Some((target, from + dir * distance))
}
