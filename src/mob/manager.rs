use rustc_hash::{FxHashMap, FxHashSet};

use petramond_math::math::IVec3;
#[cfg(test)]
use petramond_math::math::Vec3;
use petramond_world::block::Block;
use petramond_world::chunk::ChunkPos;

use super::noise::{Noise, NoiseField};
use super::spatial::MobSnapshot;
use super::{
    append_body_supports, body_has_peer_support, def, instance, solid_boxes,
    terrain_safe_motion_prefix, BodyMotion, EntityRef, Instance, MobCollision, MobId, MobRng,
};

#[cfg(test)]
use super::body_separation;

mod control;
mod drops;
mod lifecycle;
mod lod;
mod simulation;
#[cfg(test)]
mod tests;

pub use drops::{DeathDrop, MobSpill, ShearDrop};
pub use lod::SimDistance;
mod push;
mod solids;
use push::PushScratch;
pub use simulation::{MobAttack, MobExposureDamage, MobFall, MobTickEvents, PlayerAnchor};
use solids::SolidScratch;

fn nearest_anchor(
    anchors: &[PlayerAnchor],
    pos: petramond_math::world_pos::WorldPos,
) -> &PlayerAnchor {
    debug_assert!(!anchors.is_empty(), "at least the local session anchors");
    let mut best = &anchors[0];
    let mut best_d = best.pos.distance_squared(pos);
    for a in &anchors[1..] {
        let d = a.pos.distance_squared(pos);
        if d < best_d {
            best = a;
            best_d = d;
        }
    }
    best
}

const SPAWN_RNG_SALT: u64 = 0x5EED_5EED_5EED_5EED;

pub struct Mobs {
    list: Vec<Instance>,
    slot_by_id: FxHashMap<MobId, usize>,
    spawn_counter: u64,
    rng: MobRng,
    sim_distance: SimDistance,
    step_scratch: Vec<lod::SimStep>,
    turns: Vec<simulation::Turn>,
    push: PushScratch,
    solid: SolidScratch,
    scripted_requests: Vec<crate::modding::ai::AiNodeRequest>,
    path_budget: super::nav::PathBudget,
    ai_snapshot: MobSnapshot,
    exposure_scratch: crate::exposure::ExposureScratch,
    pending_noises: Vec<Noise>,
    heard: NoiseField,
    populate_checked: FxHashSet<ChunkPos>,
    confined_regions: super::confined::RegionCache,
    change_seq: u64,
    spills: Vec<drops::MobSpill>,
}

impl Default for Mobs {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Mobs {
    pub fn tick_damage_immunity(&mut self) {
        for mob in &mut self.list {
            mob.tick_damage_immunity();
        }
    }
    pub fn new(seed: u64) -> Self {
        Mobs {
            list: Vec::new(),
            slot_by_id: FxHashMap::default(),
            spawn_counter: 0,
            rng: MobRng::new(seed ^ SPAWN_RNG_SALT),
            sim_distance: SimDistance::UNLIMITED,
            step_scratch: Vec::new(),
            turns: Vec::new(),
            push: PushScratch::default(),
            solid: SolidScratch::default(),
            scripted_requests: Vec::new(),
            path_budget: super::nav::PathBudget::default(),
            ai_snapshot: MobSnapshot::default(),
            exposure_scratch: Default::default(),
            pending_noises: Vec::new(),
            heard: NoiseField::default(),
            populate_checked: FxHashSet::default(),
            confined_regions: super::confined::RegionCache::default(),
            change_seq: 0,
            spills: Vec::new(),
        }
    }

    pub fn take_spills(&mut self) -> Vec<drops::MobSpill> {
        std::mem::take(&mut self.spills)
    }

    fn spill_container(&mut self, slot: usize) {
        let Some(mob) = self.list.get_mut(slot) else {
            return;
        };
        let stacks = mob.take_container_items();
        if !stacks.is_empty() {
            self.spills.push(drops::MobSpill {
                pos: mob.pos,
                stacks,
                skylight: mob.skylight,
                blocklight: mob.blocklight,
            });
        }
    }

    pub fn change_seq(&self) -> u64 {
        self.change_seq
    }

    pub fn invalidate_confined_regions(
        &mut self,
        next: u64,
        changed: &[petramond_math::math::IVec3],
        all: bool,
    ) {
        self.change_seq = next;
        self.confined_regions.invalidate(changed, all);
    }

    pub fn push_noise(&mut self, noise: Noise) {
        self.pending_noises.push(noise);
    }

    pub fn discard_noises(&mut self) {
        self.pending_noises.clear();
        self.heard.clear();
    }

    pub fn set_sim_distance(&mut self, policy: SimDistance) {
        self.sim_distance = policy.sanitized();
    }

    pub fn sim_distance(&self) -> SimDistance {
        self.sim_distance
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    #[inline]
    fn slot(&self, id: MobId) -> Option<usize> {
        self.slot_by_id.get(&id).copied()
    }

    fn mob_mut(&mut self, id: MobId) -> Option<&mut Instance> {
        let slot = self.slot(id)?;
        self.list.get_mut(slot)
    }

    #[inline]
    pub fn get(&self, id: MobId) -> Option<&Instance> {
        self.list.get(self.slot(id)?)
    }

    #[inline]
    pub fn contains(&self, id: MobId) -> bool {
        self.slot_by_id.contains_key(&id)
    }

    #[inline]
    pub fn live(&self, id: MobId) -> Option<&Instance> {
        self.get(id).filter(|m| !m.is_dead())
    }

    #[inline]
    pub fn position_of(&self, id: MobId) -> Option<usize> {
        self.slot(id)
    }

    pub fn advance_dig(
        &mut self,
        id: MobId,
        now: u64,
        pos: IVec3,
        block: petramond_world::block::Block,
        tool: Option<petramond_world::item::Tool>,
    ) -> Option<super::DigStep> {
        Some(self.mob_mut(id)?.advance_dig(now, pos, block, tool))
    }

    #[cfg(test)]
    pub fn set_gaze_for_test(&mut self, id: MobId, yaw: f32, pitch: f32) {
        if let Some(mob) = self.mob_mut(id) {
            mob.yaw = yaw;
            mob.head_yaw = 0.0;
            mob.head_pitch = pitch;
        }
    }

    pub fn set_draw(&mut self, id: MobId, draw: crate::world::draw::BodyDraw) {
        if let Some(mob) = self.mob_mut(id) {
            mob.set_draw(draw);
        }
    }

    pub fn set_held(&mut self, id: MobId, held: [Option<petramond_world::item::ItemType>; 2]) {
        if let Some(mob) = self.mob_mut(id) {
            mob.set_held(held);
        }
    }

    pub fn container_mut(
        &mut self,
        id: MobId,
    ) -> Option<&mut petramond_world::container::Container> {
        self.mob_mut(id).map(Instance::container_mut)
    }

    #[inline]
    pub fn instances(&self) -> &[Instance] {
        &self.list
    }

    fn push_instance(&mut self, mob: Instance) {
        self.slot_by_id.insert(mob.id(), self.list.len());
        self.list.push(mob);
    }

    fn swap_remove_instance(&mut self, slot: usize) -> Instance {
        let mob = self.list.swap_remove(slot);
        self.slot_by_id.remove(&mob.id());
        if let Some(moved) = self.list.get(slot) {
            self.slot_by_id.insert(moved.id(), slot);
        }
        mob
    }

    fn retain_instances(&mut self, keep: impl FnMut(&Instance) -> bool) {
        let before = self.list.len();
        self.list.retain(keep);
        if self.list.len() != before {
            self.slot_by_id.clear();
            self.slot_by_id
                .extend(self.list.iter().enumerate().map(|(i, m)| (m.id(), i)));
        }
    }

    pub fn any_overlapping_placement(&self, p: IVec3, block: Block) -> bool {
        self.any_overlapping_boxes(p, block.collision_boxes())
    }

    pub fn any_overlapping_boxes(&self, p: IVec3, boxes: &[petramond_world::block::Aabb]) -> bool {
        self.list
            .iter()
            .filter(|m| !m.is_dead())
            .any(|m| super::body_overlaps_block_boxes(m.pos, m.yaw, def(m.kind).size, p, boxes))
    }
}
