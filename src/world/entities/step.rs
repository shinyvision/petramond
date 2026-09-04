//! How one item entity advances a world tick: each motion variant's step,
//! composed from the entity's own physics (`entity::DroppedItem`) and what
//! only the world store can see — the live bodies a flight sweeps, and the
//! announced block changes a lodged item watches its anchor through.

use std::collections::HashSet;

use crate::entity::{DroppedItem, Motion};
use crate::mob::{EntityRef, PlayerAnchor};
use crate::world::World;
use petramond_math::math::{IVec3, Vec3};

use super::sweep::{sweep, SweepBodies};
use super::ItemImpact;

/// How far behind the impact point a striking item's centre comes to rest,
/// in blocks: the head buried, the tail standing proud of the surface.
const IMPACT_SEAT: f32 = 0.15;

/// Past this many announced cells the anchor test hashes them rather than
/// scanning: a wall of lodged items against a bulk edit must not go
/// quadratic.
const CHANGED_HASH_THRESHOLD: usize = 16;

/// The announced block changes since the last item tick, shaped as the one
/// question a lodged item asks of them: was my anchor touched?
pub(super) enum ChangedCells<'a> {
    /// The feed overflowed: every cell may have changed.
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

/// What every item's step shares this tick beyond the entity itself.
pub(super) struct StepCtx<'a> {
    pub world: &'a World,
    pub dt: f32,
    pub anchors: &'a [PlayerAnchor],
    pub changed: ChangedCells<'a>,
    /// The mobs a flight may strike, bucketed for the sweep; gathered only
    /// while something is in flight.
    pub bodies: Option<SweepBodies>,
}

impl StepCtx<'_> {
    /// Whether the segment `from → from + motion` meets `body`'s box —
    /// starts inside it, or crosses it. A body that is gone (a spectator, a
    /// departed session, a dead or unloaded mob) meets nothing.
    fn segment_meets_body(&self, body: EntityRef, from: Vec3, motion: Vec3) -> bool {
        let meets = |lo: Vec3, hi: Vec3| segment_meets_box(from, motion, lo, hi);
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

    /// The body centre a requested drop magnets toward: ITS requester's —
    /// never someone else's, so two players vacuuming side by side each
    /// pull their own reservations.
    fn magnet_for(&self, item: &DroppedItem) -> Option<Vec3> {
        let by = item.pickup_requested?;
        self.anchors.iter().find(|a| a.id == by).map(|a| a.pos)
    }
}

fn contains(lo: Vec3, hi: Vec3, p: Vec3) -> bool {
    (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z)
}

/// Whether the segment `from → from + motion` starts inside or crosses the
/// box `lo..hi`.
fn segment_meets_box(from: Vec3, motion: Vec3, lo: Vec3, hi: Vec3) -> bool {
    if contains(lo, hi, from) {
        return true;
    }
    let length = motion.length();
    length > 1e-6
        && crate::player::ray_vs_aabb(from, motion / length, lo, hi).is_some_and(|t| t <= length)
}

impl DroppedItem {
    /// Advance this item one tick by its motion's own step. Only a flight
    /// has anything to report: the impact that stopped it, for the stage
    /// owner to resolve.
    pub(super) fn step(&mut self, ctx: &StepCtx) -> Option<ItemImpact> {
        match self.motion {
            Motion::Loose => {
                self.step_loose(ctx.dt, ctx.world, ctx.magnet_for(self));
                None
            }
            Motion::Flight(_) => self.step_flight(ctx),
            // A lodged item a player reaches for comes loose into the magnet
            // like any drop; otherwise it holds while its block does.
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

    /// The FLIGHT step: integrate the velocity, then sweep this tick's
    /// motion for the first body or block on it. A strike seats the item at
    /// the impact — still in flight; what happens next is the server's.
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

    /// The STUCK step: hold still; probe the anchor only when it was
    /// announced changed, or once after a reload (the block may have gone
    /// while the section was out). The probe waits for the anchor's column
    /// to be loaded — an unloaded column reads as empty and would release a
    /// lodged item that has lost nothing.
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
        if !ctx.world.chunk_loaded(a.x >> 4, a.z >> 4) {
            return;
        }
        if ctx.world.collision_boxes_at(a.x, a.y, a.z).is_empty() {
            self.release();
        } else if let Motion::Stuck(stuck) = &mut self.motion {
            stuck.verified = true;
        }
    }
}
