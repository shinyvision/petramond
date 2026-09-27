use std::collections::HashMap;

use crate::mob::{populate, spawn, Instance, Mob, MobId, SavedMob};
use crate::world::ServerWorld;
use petramond_math::math::Vec3;
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::Mobs;

impl Mobs {
    pub fn spawn(&mut self, kind: Mob, pos: petramond_math::world_pos::WorldPos, yaw: f32) -> bool {
        self.spawn_lit(
            kind,
            pos,
            yaw,
            63,
            petramond_world::light::BlockLight6::DARK,
        )
        .is_some()
    }

    pub fn spawn_lit(
        &mut self,
        kind: Mob,
        pos: petramond_math::world_pos::WorldPos,
        yaw: f32,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
    ) -> Option<MobId> {
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut mob = Instance::new(kind, pos, yaw, self.spawn_counter);
        mob.skylight = skylight;
        mob.blocklight = blocklight;
        let id = mob.id();
        self.push_instance(mob);
        Some(id)
    }

    pub fn spawn_room_for(&self, kind: Mob) -> u32 {
        spawn::room_for(&self.list, kind)
    }

    pub fn spawn_tick(
        &mut self,
        world: &ServerWorld,
        player_pos: petramond_math::world_pos::WorldPos,
    ) -> Vec<(u64, Mob, petramond_math::world_pos::WorldPos)> {
        let list = &self.list;
        let chosen = spawn::attempt(world, player_pos, &mut self.rng, |kind| {
            spawn::room_for(list, kind)
        });
        let mut spawned = Vec::new();
        if let Some(spawns) = chosen {
            for s in spawns {
                let c = (s.pos + Vec3::new(0.0, 0.3, 0.0)).block();
                let sky = world.data().skylight6_at_world(c.x, c.y, c.z);
                let block = petramond_world::light::BlockLight6::from_x2(
                    world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                );
                if let Some(id) = self.spawn_lit(s.kind, s.pos, s.yaw, sky, block) {
                    spawned.push((id, s.kind, s.pos));
                }
            }
        }
        spawned
    }

    pub fn populate_tick(
        &mut self,
        world: &ServerWorld,
        player_pos: petramond_math::world_pos::WorldPos,
    ) -> (
        Vec<(u64, Mob, petramond_math::world_pos::WorldPos)>,
        Vec<ChunkPos>,
    ) {
        let herds = populate::attempt(world, player_pos, &mut self.populate_checked);
        let mut spawned = Vec::new();
        let mut populated = Vec::new();
        for herd in herds {
            let mut any = false;
            for s in herd.spawns {
                let c = (s.pos + Vec3::new(0.0, 0.3, 0.0)).block();
                let sky = world.data().skylight6_at_world(c.x, c.y, c.z);
                let block = petramond_world::light::BlockLight6::from_x2(
                    world.data().blocklight_rgb_at_world(c.x, c.y, c.z),
                );
                if let Some(id) = self.spawn_lit(s.kind, s.pos, s.yaw, sky, block) {
                    spawned.push((id, s.kind, s.pos));
                    any = true;
                }
            }
            if any {
                populated.push(herd.chunk);
            }
        }
        (spawned, populated)
    }

    pub fn take_in_section(&mut self, pos: SectionPos) -> Vec<SavedMob> {
        let mut taken = Vec::new();
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let c = self.list[i].pos.block();
            if SectionPos::from_world(c.x, c.y, c.z) == Some(pos) {
                let mob = self.swap_remove_instance(i);
                if !mob.is_dead() {
                    taken.push(SavedMob::of(&mob));
                }
            }
        }
        taken
    }

    pub fn saved_by_section(&self) -> HashMap<SectionPos, Vec<SavedMob>> {
        let mut map: HashMap<SectionPos, Vec<SavedMob>> = HashMap::new();
        for m in &self.list {
            if m.is_dead() {
                continue;
            }
            let c = m.pos.block();
            if let Some(pos) = SectionPos::from_world(c.x, c.y, c.z) {
                map.entry(pos).or_default().push(SavedMob::of(m));
            }
        }
        map
    }

    pub fn restore(&mut self, mobs: impl IntoIterator<Item = SavedMob>) {
        for m in mobs {
            self.restore_saved_mob_lit(m, 63, petramond_world::light::BlockLight6::DARK);
        }
    }

    pub fn restore_saved_mob_lit(
        &mut self,
        m: SavedMob,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
    ) {
        let id = self.spawn_lit(m.kind, m.pos, m.yaw, skylight, blocklight);
        if let Some(inst) = id.and_then(|id| self.mob_mut(id)) {
            inst.overlay_tags(m.tags);
            inst.restore_container(m.container);
        }
    }

    pub fn remove(&mut self, id: MobId) -> bool {
        let Some(slot) = self.slot(id) else {
            return false;
        };
        self.spill_container(slot);
        self.swap_remove_instance(slot);
        true
    }
}
