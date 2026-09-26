//! The soft-push pass: overlapping bodies drift apart through per-tick push
//! velocities, and the same overlap tests record the TOUCH perception channel.

use petramond_math::math::Vec3;
use petramond_world::body::Body;

use super::{def, EntityRef, Instance, Mobs, PlayerAnchor};

/// One participating body's push geometry.
#[derive(Copy, Clone)]
pub(super) struct PushBody {
    pos: petramond_math::world_pos::WorldPos,
    yaw: f32,
    size: super::super::MobSize,
    /// Horizontal radius past which this body provably cannot overlap another
    /// (mirrors `body_geometry`'s compound bound), for the sweep broadphase.
    reach: f32,
}

/// The push pass's reused buffers (every one index-aligned with the live set
/// unless noted).
#[derive(Default)]
pub(super) struct PushScratch {
    bodies: Vec<Option<PushBody>>,
    velocities: Vec<Vec3>,
    /// Participants in stable-id order.
    order: Vec<usize>,
    /// The broadphase's x-sorted sweep list and the candidate pairs it yields.
    sweep: Vec<(f64, u32)>,
    pairs: Vec<(u32, u32)>,
    ids: Vec<u64>,
    contacts: Vec<Vec<EntityRef>>,
}

/// The horizontal radius `body_geometry`'s compound push bound uses. Kept
/// identical to it: the sweep is only sound while it is an upper bound on the
/// narrow phase's own reach.
fn push_reach(size: super::super::MobSize) -> f32 {
    size.half_length
        .unwrap_or(size.half_width)
        .hypot(size.half_width)
}

#[cfg(test)]
impl PushBody {
    pub(super) fn pos(self) -> petramond_math::world_pos::WorldPos {
        self.pos
    }
    pub(super) fn yaw(self) -> f32 {
        self.yaw
    }
    pub(super) fn size(self) -> super::super::MobSize {
        self.size
    }
}

/// Build a push body from raw geometry — the broadphase test's fixture.
#[cfg(test)]
pub(super) fn push_body_for_test(
    pos: petramond_math::world_pos::WorldPos,
    yaw: f32,
    size: super::super::MobSize,
) -> PushBody {
    PushBody {
        pos,
        yaw,
        size,
        reach: push_reach(size),
    }
}

/// Every pair of participating bodies (as ranks into `order`) whose push
/// bounds could overlap, in the same order the exhaustive nested scan visits
/// them: `(rank_a, rank_b)` ascending, `rank_a < rank_b`.
///
/// The sweep is one axis: sorted by x, a pair is only possible while the x gap
/// is under the two bodies' combined horizontal reach, and the widest body in
/// the set bounds the partner's half. That bound is exactly the one
/// `body_geometry`'s compound test applies, so no overlapping pair can be
/// missed — the narrow phase still runs on every candidate and decides.
pub(super) fn overlap_pairs(
    bodies: &[Option<PushBody>],
    order: &[usize],
    sweep: &mut Vec<(f64, u32)>,
    pairs: &mut Vec<(u32, u32)>,
) {
    sweep.clear();
    pairs.clear();
    let mut max_reach = 0.0f32;
    for (rank, &i) in order.iter().enumerate() {
        let body = bodies[i].expect("order holds participating bodies only");
        max_reach = max_reach.max(body.reach);
        sweep.push((body.pos.x, rank as u32));
    }
    sweep.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    for p in 0..sweep.len() {
        let (x, ra) = sweep[p];
        let span = bodies[order[ra as usize]].expect("participating").reach + max_reach;
        for &(qx, rb) in &sweep[p + 1..] {
            if qx - x >= f64::from(span) {
                break;
            }
            pairs.push((ra.min(rb), ra.max(rb)));
        }
    }
    pairs.sort_unstable();
}

impl Mobs {
    /// Soft-push pass: for every overlapping pair of bodies — mob↔mob, and mob←player when
    /// `player` is present — set each mob's push *velocity* away from the others, to be
    /// applied (through the mob's own collision) on its next integrate. This shoves only
    /// *mobs* (which simulate on the tick); the player's own pushback is computed
    /// per-frame in [`push_on_player`](Self::push_on_player), not here.
    ///
    /// Computed from a single up-front snapshot of every compound body. Each mob pair is
    /// visited once and its deepest segment overlap yields one equal-and-opposite
    /// separation, so segment count and list order cannot multiply the shove. A mob that
    /// isn't pushable this tick (dead, or frozen over an unloaded chunk or by simulation
    /// distance) neither pushes nor is pushed.
    /// The same overlap tests double as the TOUCH perception channel: every
    /// overlapping entity is recorded on the mob as a contact (`EntityRef`),
    /// which next tick's AI reads as `AiCtx::contacts` (the `chase_contact`
    /// node's input). A mob that doesn't participate this tick gets its
    /// contacts cleared, so nothing stales through death or a freeze.
    pub(super) fn resolve_pushes(&mut self, anchors: &[PlayerAnchor]) {
        let mut scratch = std::mem::take(&mut self.push);
        let PushScratch {
            bodies,
            velocities: pushes,
            order,
            sweep,
            pairs,
            ids,
            contacts,
        } = &mut scratch;
        // `None` marks a mob that doesn't participate this tick; index aligns with `list`.
        bodies.clear();
        // A mob pushes only while alive (a corpse ragdolls in place — its
        // `pos` is the ragdoll origin, so shoving it would warp the corpse)
        // and actually simulating this tick (not frozen over unloaded
        // terrain or by simulation distance).
        let turns = &self.turns;
        bodies.extend(self.list.iter().enumerate().map(|(i, m)| {
            (!m.is_dead() && turns[i].simulated()).then(|| {
                let size = def(m.kind).size;
                PushBody {
                    pos: m.pos,
                    yaw: m.yaw,
                    size,
                    reach: push_reach(size),
                }
            })
        }));
        ids.clear();
        ids.extend(self.list.iter().map(Instance::id));
        contacts.resize_with(self.list.len(), Vec::new);
        contacts.iter_mut().for_each(Vec::clear);
        pushes.clear();
        pushes.resize(self.list.len(), Vec3::ZERO);
        order.clear();
        order.extend((0..self.list.len()).filter(|&i| bodies[i].is_some()));
        order.sort_by_key(|&i| ids[i]);

        // Resolve each mob pair once. A compound body contributes its deepest
        // segment contact only, then the pair's one separation is applied
        // equally and oppositely to whichever members are soft.
        //
        // The candidate pairs come from a one-axis sweep, not the full N²
        // scan: bodies are spread over the loaded world and the overlapping
        // pairs are a vanishing fraction of them, while the N² form doubles
        // its cost every time the population does. The pairs are re-sorted
        // into the same (rank, rank) order the nested scan visited, so the
        // accumulated push sums and the contact lists are unchanged.
        overlap_pairs(bodies, order, sweep, pairs);
        for &(ra, rb) in pairs.iter() {
            let (i, j) = (order[ra as usize], order[rb as usize]);
            let (a, b) = (bodies[i].unwrap(), bodies[j].unwrap());
            let Some(push_a) = super::super::body_separation(a.pos, a.yaw, a.size, b.pos, b.yaw, b.size)
            else {
                continue;
            };
            if def(self.list[i].kind).collision != super::super::MobCollision::Solid {
                pushes[i] += push_a;
            }
            if def(self.list[j].kind).collision != super::super::MobCollision::Solid {
                pushes[j] -= push_a;
            }
            contacts[i].push(EntityRef::Mob(ids[j]));
            contacts[j].push(EntityRef::Mob(ids[i]));
        }

        // Players are ordinary one-box bodies. Solid mobs still record touch
        // but receive no soft push, and the reverse player reaction remains a
        // per-frame query below.
        for (i, body) in bodies.iter().copied().enumerate() {
            let Some(body) = body else {
                continue;
            };
            let rigid = def(self.list[i].kind).collision == super::super::MobCollision::Solid;
            for anchor in anchors {
                let Some(player) = anchor.body else {
                    continue;
                };
                if let Some(push) =
                    super::super::body_separation_from_body(body.pos, body.yaw, body.size, player)
                {
                    if !rigid {
                        pushes[i] += push;
                    }
                    contacts[i].push(EntityRef::Player(anchor.id));
                }
            }
        }

        for i in 0..self.list.len() {
            self.list[i].set_push(pushes[i]);
            self.list[i].set_contacts(contacts[i].drain(..));
        }
        self.push = scratch;
    }

    /// The net horizontal push *velocity* the live mobs impart on the player right now,
    /// from the player's current body — read-only, mutating no mob. The caller applies it
    /// to the player **per-frame** (not on the tick) so the player drifts out of an
    /// overlap perfectly smoothly: player movement is integrated every frame, and a 20 Hz
    /// shove would pulse. A dead mob (a ragdolling corpse) doesn't push; a frozen mob over
    /// an unloaded chunk is far from the player and never overlaps, so it's moot here.
    pub fn push_on_player(&self, player: Body) -> Vec3 {
        let mut push = Vec3::ZERO;
        for m in &self.list {
            // A SOLID body never soft-pushes the player: it is a rigid
            // obstacle in the player's own resolver — a push would fight the
            // contact (skating a stander off the deck).
            if m.is_dead() || def(m.kind).collision == super::super::MobCollision::Solid {
                continue;
            }
            let d = def(m.kind);
            if let Some(mob_push) = super::super::body_separation_from_body(m.pos, m.yaw, d.size, player) {
                push -= mob_push;
            }
        }
        push
    }
}
