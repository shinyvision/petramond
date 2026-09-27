use crate::world::ServerWorld;
use std::collections::BTreeSet;

use crate::mob::{Mobs, SavedMob};
use petramond_math::math::Vec3;
use petramond_world::chunk::ChunkPos;

impl ServerWorld {
    #[inline]
    pub fn mobs(&self) -> &Mobs {
        &self.side.entities.mobs
    }

    #[inline]
    pub fn mobs_mut(&mut self) -> &mut Mobs {
        &mut self.side.entities.mobs
    }

    pub fn spawn_mob(
        &mut self,
        kind: crate::mob::Mob,
        pos: petramond_math::world_pos::WorldPos,
        yaw: f32,
    ) -> Option<u64> {
        let (sky, block) = self.mob_render_light_at(pos);
        self.side
            .entities
            .mobs
            .spawn_lit(kind, pos, yaw, sky, block)
    }

    pub fn spawn_mob_checked(
        &mut self,
        kind: crate::mob::Mob,
        pos: petramond_math::world_pos::WorldPos,
        yaw: f32,
    ) -> Option<u64> {
        if !self.mob_spawn_pose_clear(kind, pos, yaw) {
            return None;
        }
        self.spawn_mob(kind, pos, yaw)
    }

    pub fn mob_spawn_pose_clear(
        &self,
        kind: crate::mob::Mob,
        pos: petramond_math::world_pos::WorldPos,
        yaw: f32,
    ) -> bool {
        let obstacles = self.side.entities.mobs.solid_obstacles();
        crate::mob::body_pose_fits(
            pos,
            yaw,
            crate::mob::def(kind).size,
            &|x, y, z| self.data.collision_boxes_at(x, y, z),
            &|x, y, z| self.section_stream_final_at(x, y, z),
            &obstacles,
        )
    }

    pub fn restore_mobs(&mut self, mobs: impl IntoIterator<Item = SavedMob>) {
        for mob in mobs {
            let (sky, block) = self.mob_render_light_at(mob.pos);
            self.side
                .entities
                .mobs
                .restore_saved_mob_lit(mob, sky, block);
        }
    }

    fn mob_render_light_at(
        &self,
        pos: petramond_math::world_pos::WorldPos,
    ) -> (u8, petramond_world::light::BlockLight6) {
        let c = (pos + Vec3::new(0.0, 0.3, 0.0)).block();
        let sky = self.data.skylight6_at_world(c.x, c.y, c.z);
        let block = petramond_world::light::BlockLight6::from_x2(
            self.data.blocklight_rgb_at_world(c.x, c.y, c.z),
        );
        (sky, block)
    }

    pub fn push_noise(&mut self, noise: crate::mob::Noise) {
        self.side.entities.mobs.push_noise(noise);
    }

    #[inline]
    pub fn riding(&self) -> &crate::mob::riding::Riding {
        &self.side.entities.riding
    }

    #[inline]
    pub fn riding_mut(&mut self) -> &mut crate::mob::riding::Riding {
        &mut self.side.entities.riding
    }

    pub fn try_mount_player(&mut self, player: u8, mob_id: u64, seat: u8) -> bool {
        let Some(mob) = self.side.entities.mobs.live(mob_id) else {
            return false;
        };
        if seat as usize >= crate::mob::def(mob.kind).seats.len() {
            return false;
        }
        self.side
            .entities
            .riding
            .mount(player, crate::mob::riding::MountTarget::Mob(mob_id), seat)
    }

    pub fn try_mount_anchor(&mut self, player: u8, anchor: crate::mob::riding::PoseAnchor) -> bool {
        self.side
            .entities
            .riding
            .mount(player, crate::mob::riding::MountTarget::Anchor(anchor), 0)
    }

    pub fn tick_mobs(
        &mut self,
        dt: f32,
        anchors: &[crate::mob::PlayerAnchor],
    ) -> crate::mob::MobTickEvents {
        self.route_probe_budget().refill();
        let (next, changed, lost) = self.nav_changes_since(self.side.entities.mobs.change_seq());
        self.side
            .entities
            .mobs
            .invalidate_confined_regions(next, &changed, lost);
        if self.side.entities.mobs.is_empty() {
            self.side.entities.mobs.discard_noises();
            return crate::mob::MobTickEvents::default();
        }
        self.reach_budget().refill();
        let freeze_unloaded = self.side.save.is_some();
        let mut mobs = std::mem::take(&mut self.side.entities.mobs);
        let attacks = mobs.tick(dt, self, anchors, freeze_unloaded);
        self.side.entities.mobs = mobs;
        attacks
    }

    pub fn spawn_mobs_tick(
        &mut self,
        player_pos: petramond_math::world_pos::WorldPos,
    ) -> Vec<(u64, crate::mob::Mob, petramond_math::world_pos::WorldPos)> {
        let mut mobs = std::mem::take(&mut self.side.entities.mobs);
        let spawned = mobs.spawn_tick(self, player_pos);
        self.side.entities.mobs = mobs;
        spawned
    }

    pub fn populate_mobs_tick(
        &mut self,
        player_pos: petramond_math::world_pos::WorldPos,
    ) -> Vec<(u64, crate::mob::Mob, petramond_math::world_pos::WorldPos)> {
        let mut mobs = std::mem::take(&mut self.side.entities.mobs);
        let (spawned, populated) = mobs.populate_tick(self, player_pos);
        self.side.entities.mobs = mobs;
        self.side.gen.populated_columns.extend(populated);
        spawned
    }

    pub fn column_populated(&self, chunk: ChunkPos) -> bool {
        self.side.gen.populated_columns.contains(&chunk)
    }

    pub fn populated_columns(&self) -> &BTreeSet<ChunkPos> {
        &self.side.gen.populated_columns
    }

    pub fn set_populated_columns(&mut self, set: BTreeSet<ChunkPos>) {
        self.side.gen.populated_columns = set;
    }
}
