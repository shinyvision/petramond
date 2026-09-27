use petramond_math::math::Vec3;
use petramond_world::body::Body;

use super::{def, EntityRef, Instance, Mobs, PlayerAnchor};

#[derive(Copy, Clone)]
pub(super) struct PushBody {
    pos: petramond_math::world_pos::WorldPos,
    yaw: f32,
    size: super::super::MobSize,
    reach: f32,
}

#[derive(Default)]
pub(super) struct PushScratch {
    bodies: Vec<Option<PushBody>>,
    velocities: Vec<Vec3>,
    order: Vec<usize>,
    sweep: Vec<(f64, u32)>,
    pairs: Vec<(u32, u32)>,
    ids: Vec<u64>,
    contacts: Vec<Vec<EntityRef>>,
}

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
    /// Soft-push pass: for overlapping pairs (mob↔mob, mob←player if present), sets each mob's push
    /// velocity away from the others, applied through the mob's own collision on its next
    /// integrate. Only mobs get shoved here; the player's pushback is per-frame in
    /// [`push_on_player`](Self::push_on_player).
    ///
    /// Uses one snapshot of all bodies. Each pair is visited once, and its deepest segment overlap
    /// gives one equal-and-opposite separation, so segment count or list order can't stack the
    /// shove. Dead or frozen (unloaded chunk, sim distance) mobs neither push nor get pushed.
    ///
    /// The same overlap tests feed touch perception: overlapping entities are recorded as contacts
    /// on the mob, read next tick as `AiCtx::contacts` (the `chase_contact` node's input). Mobs
    /// that don't participate get their contacts cleared, so nothing stales through death or a
    /// freeze.
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
        bodies.clear();
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

        overlap_pairs(bodies, order, sweep, pairs);
        for &(ra, rb) in pairs.iter() {
            let (i, j) = (order[ra as usize], order[rb as usize]);
            let (a, b) = (bodies[i].unwrap(), bodies[j].unwrap());
            let Some(push_a) =
                super::super::body_separation(a.pos, a.yaw, a.size, b.pos, b.yaw, b.size)
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

    pub fn push_on_player(&self, player: Body) -> Vec3 {
        let mut push = Vec3::ZERO;
        for m in &self.list {
            if m.is_dead() || def(m.kind).collision == super::super::MobCollision::Solid {
                continue;
            }
            let d = def(m.kind);
            if let Some(mob_push) =
                super::super::body_separation_from_body(m.pos, m.yaw, d.size, player)
            {
                push -= mob_push;
            }
        }
        push
    }
}
