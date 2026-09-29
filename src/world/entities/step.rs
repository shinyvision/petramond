use crate::world::ServerWorld;
use std::collections::HashSet;

use crate::entity::{DroppedItem, Motion};
use crate::mob::{EntityRef, PlayerAnchor};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;

use super::sweep::{sweep, SweepBodies};
use super::ItemImpact;

const IMPACT_SEAT: f32 = 0.15;
const DISLODGE_HORIZONTAL_SPEED: f32 = 1.5;

const CHANGED_HASH_THRESHOLD: usize = 16;

pub(super) enum ChangedCells<'a> {
    All,
    Few(&'a [IVec3]),
    Many(HashSet<IVec3>),
}

impl<'a> ChangedCells<'a> {
    pub(super) fn new(changed: &'a [IVec3], overflow: bool) -> Self {
        if overflow {
            ChangedCells::All
        } else if changed.len() > CHANGED_HASH_THRESHOLD {
            ChangedCells::Many(changed.iter().copied().collect())
        } else {
            ChangedCells::Few(changed)
        }
    }

    pub(super) fn contains(&self, cell: IVec3) -> bool {
        match self {
            ChangedCells::All => true,
            ChangedCells::Few(cells) => cells.contains(&cell),
            ChangedCells::Many(cells) => cells.contains(&cell),
        }
    }
}

pub(super) struct StepCtx<'a> {
    pub world: &'a ServerWorld,
    pub dt: f32,
    pub anchors: &'a [PlayerAnchor],
    pub changed: ChangedCells<'a>,
    pub bodies: Option<SweepBodies>,
}

impl StepCtx<'_> {
    fn segment_meets_body(&self, body: EntityRef, from: WorldPos, motion: Vec3) -> bool {
        let rel = |p: [f64; 3]| {
            Vec3::new(
                (p[0] - from.x) as f32,
                (p[1] - from.y) as f32,
                (p[2] - from.z) as f32,
            )
        };
        let meets =
            |lo: [f64; 3], hi: [f64; 3]| segment_meets_box(Vec3::ZERO, motion, rel(lo), rel(hi));
        match body {
            EntityRef::Player(id) => self
                .anchors
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.body)
                .is_some_and(|b| {
                    let (lo, hi) = b.aabb();
                    meets(lo, hi)
                }),
            EntityRef::Mob(id) => self
                .world
                .mobs()
                .instances()
                .iter()
                .find(|m| m.id() == id && !m.is_dead())
                .is_some_and(|m| {
                    crate::mob::body_boxes(m.pos, m.yaw, crate::mob::def(m.kind).size)
                        .any(|(lo, hi)| meets(lo, hi))
                }),
        }
    }

    fn magnet_for(&self, item: &DroppedItem) -> Option<WorldPos> {
        let by = item.pickup_requested?;
        self.anchors.iter().find(|a| a.id == by).map(|a| a.pos)
    }
}

fn contains(lo: Vec3, hi: Vec3, p: Vec3) -> bool {
    (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z)
}

fn segment_meets_box(from: Vec3, motion: Vec3, lo: Vec3, hi: Vec3) -> bool {
    if contains(lo, hi, from) {
        return true;
    }
    let length = motion.length();
    length > 1e-6
        && petramond_world::world::raycast::ray_vs_aabb(from, motion / length, lo, hi)
            .is_some_and(|t| t <= length)
}

impl DroppedItem {
    pub(super) fn step(&mut self, ctx: &StepCtx) -> Option<ItemImpact> {
        match self.motion {
            Motion::Loose => {
                self.step_loose(ctx.dt, ctx.world, ctx.magnet_for(self));
                None
            }
            Motion::Flight(_) => self.step_flight(ctx),
            Motion::Stuck(_) if self.pickup_requested.is_some() => {
                self.release();
                self.step_loose(ctx.dt, ctx.world, ctx.magnet_for(self));
                None
            }
            Motion::Stuck(_) => {
                self.step_stuck(ctx);
                None
            }
        }
    }

    fn step_flight(&mut self, ctx: &StepCtx) -> Option<ItemImpact> {
        let motion = self.advance_flight(ctx.dt)?;
        let Motion::Flight(flight) = &mut self.motion else {
            return None;
        };
        // The launcher is spared until one whole tick's segment is clear of
        // its body: a launch starts inside (or beside) the archer, and a
        // body walking into its own slow shot keeps meeting it. Only a shot
        // that has been genuinely clear once can come back and strike.
        if !flight.left_owner {
            flight.left_owner = !flight
                .owner
                .is_some_and(|owner| ctx.segment_meets_body(owner, self.pos, motion));
        }
        let spared = if flight.left_owner {
            None
        } else {
            flight.owner
        };
        let Some((target, point)) = sweep(ctx, self.pos, motion, spared) else {
            self.pos += motion;
            return None;
        };
        self.pos = point - motion.normalize_or_zero() * IMPACT_SEAT;
        Some(ItemImpact {
            id: self.id,
            target,
            point,
            vel: self.vel,
        })
    }

    fn step_stuck(&mut self, ctx: &StepCtx) {
        self.prev_pos = self.pos;
        self.prev_spin = self.spin;
        let Motion::Stuck(stuck) = self.motion else {
            return;
        };
        if stuck.verified && !ctx.changed.contains(stuck.anchor) {
            return;
        }
        let a = stuck.anchor;
        if !ctx.world.data.chunk_loaded(a.x >> 4, a.z >> 4) {
            return;
        }
        if ctx.world.data.collision_boxes_at(a.x, a.y, a.z).is_empty() {
            // Forward drift lets gravity tip a slanted projectile down over time.
            let heading = stuck.flight.heading.dir();
            let horizontal = Vec3::new(heading.x, 0.0, heading.z);
            self.vel = if horizontal.length_squared() > 1e-12 {
                horizontal * DISLODGE_HORIZONTAL_SPEED
            } else {
                Vec3::ZERO
            };
            self.motion = Motion::Flight(stuck.flight);
        } else if let Motion::Stuck(stuck) = &mut self.motion {
            stuck.verified = true;
        }
    }
}
