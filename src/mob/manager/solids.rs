//! Solid-collision bodies (`mobs.json` `"collision": "solid"`): the obstacle
//! boxes every body resolves against, and the simultaneous peer-motion solve
//! that commits every solid's proposal together, independent of storage order.

use petramond_world::collision::DynBox;

use super::{def, BodyMotion, MobCollision, Mobs};
use crate::world::ServerWorld;

/// The solid solve's reused buffers.
#[derive(Default)]
pub(super) struct SolidScratch {
    /// Terrain-resolved solid-body proposals, stable-id sorted before the
    /// pair solver runs, with their live-set slots.
    motions: Vec<BodyMotion>,
    slots: Vec<usize>,
    limits: Vec<f32>,
    checked: Vec<f32>,
    /// The exact peer supports handed to one moving solid's proposal.
    pub supports: Vec<DynBox>,
    solver: super::super::SolidMotionSolver,
}

impl Mobs {
    /// Solve solid peers from their complete proposals, then commit every
    /// selected prefix together. Stable-id sorting keeps the broadphase and
    /// tie paths independent of storage order. `obstacles` is the
    /// start-of-tick solid box set; its buffer is reused for the committed one.
    pub(super) fn solve_solids(&mut self, world: &ServerWorld, obstacles: Vec<DynBox>) {
        let mut scratch = std::mem::take(&mut self.solid);
        let SolidScratch {
            motions,
            slots,
            limits,
            checked,
            solver,
            ..
        } = &mut scratch;
        motions.clear();
        slots.clear();
        slots.extend(self.list.iter().enumerate().filter_map(|(i, mob)| {
            (!mob.is_dead() && def(mob.kind).collision == MobCollision::Solid).then_some(i)
        }));
        slots.sort_by_key(|&i| self.list[i].id());
        motions.extend(slots.iter().map(|&i| {
            let mob = &self.list[i];
            let motion_start = self.turns[i].start.map(|(_, start)| start);
            BodyMotion {
                id: mob.id(),
                start_pos: motion_start.unwrap_or(mob.pos),
                start_yaw: if motion_start.is_some() {
                    mob.interp.yaw
                } else {
                    mob.yaw
                },
                end_pos: mob.pos,
                end_yaw: mob.yaw,
                size: def(mob.kind).size,
            }
        }));
        limits.clear();
        limits.resize(motions.len(), 1.0);
        checked.clear();
        checked.resize(motions.len(), 0.0);
        let cells = world.cursor();
        let terrain_boxes = |x: i32, y: i32, z: i32| cells.collision_boxes_xyz(x, y, z);
        let mut settled = false;
        for _ in 0..=motions.len() {
            solver.resolve_with_limits(motions, limits);
            let mut limits_changed = false;
            for (i, motion) in motions.iter().copied().enumerate() {
                let fraction = solver.fractions()[i];
                if fraction >= 1.0 - 1e-6 || fraction <= checked[i] + 1e-6 {
                    continue;
                }
                let safe = super::terrain_safe_motion_prefix(motion, fraction, &terrain_boxes);
                if safe + 1e-6 < fraction {
                    limits[i] = limits[i].min(safe);
                    checked[i] = safe;
                    limits_changed = true;
                } else {
                    checked[i] = fraction;
                }
            }
            if !limits_changed {
                settled = true;
                break;
            }
        }
        debug_assert!(settled, "each terrain limit can tighten at most once");
        for ((motion, &fraction), &i) in motions.iter().zip(solver.fractions()).zip(slots.iter()) {
            self.list[i].commit_solid_motion(*motion, fraction);
        }

        // Final top-face support is queried only after every solid has committed,
        // so landing does not depend on storage order. Supplying the same exact
        // supports to the next proposal lets gravity land first and horizontal
        // drive/AI motion continue, just like the terrain resolver's Y-then-XZ
        // ordering.
        let mut committed = obstacles;
        committed.clear();
        for mob in &self.list {
            let d = def(mob.kind);
            if !mob.is_dead() && d.collision == MobCollision::Solid {
                super::solid_boxes(mob.id(), mob.pos, mob.yaw, d.size, &mut committed);
            }
        }
        for (motion, &i) in motions.iter().zip(slots.iter()) {
            let moved = self.turns[i].start.is_some();
            if moved
                && motion.moves_down()
                && super::body_has_peer_support(
                    self.list[i].pos,
                    self.list[i].yaw,
                    motion.size,
                    &committed,
                    motion.id,
                )
            {
                self.list[i].land_on_solid_peer();
            }
        }
        self.solid = scratch;
    }

    /// The dynamic collision boxes of every LIVE solid-collision body
    /// (`mobs.json` `"collision": "solid"`) — what players and mobs resolve
    /// their movement against, beside the world's cell boxes. A dead body
    /// stops blocking (a wreck is not a wall). Long bodies emit a run of
    /// boxes along their facing (see [`super::super::solid_boxes`]).
    pub fn solid_obstacles(&self) -> Vec<DynBox> {
        let mut out = Vec::new();
        for m in &self.list {
            let d = def(m.kind);
            if !m.is_dead() && d.collision == MobCollision::Solid {
                super::solid_boxes(m.id(), m.pos, m.yaw, d.size, &mut out);
            }
        }
        out
    }
}
