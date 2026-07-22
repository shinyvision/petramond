//! Mob, riding, and population access on the world: spawn/restore funnels,
//! per-tick advancement, and the persisted worldgen-herd bookkeeping.

use std::collections::BTreeSet;

use crate::chunk::ChunkPos;
use crate::mathh::{voxel_at, Vec3};
use crate::mob::{Mobs, SavedMob};

use super::store::World;

impl World {
    /// The active mobs (read-only), for `Game` to forward to the render-side scene
    /// adapter and to ray-test for crosshair targeting.
    #[inline]
    pub fn mobs(&self) -> &Mobs {
        &self.mobs
    }

    /// Mutable access to the active mobs.
    #[inline]
    pub fn mobs_mut(&mut self) -> &mut Mobs {
        &mut self.mobs
    }

    /// Spawn a mob and initialize its cached render light immediately, so a mob
    /// created after the mob tick does not render full-bright until the next tick.
    /// Returns the newborn's stable session id, or `None` when the cap dropped it.
    pub fn spawn_mob(&mut self, kind: crate::mob::Mob, pos: Vec3, yaw: f32) -> Option<u64> {
        let (sky, block) = self.mob_render_light_at(pos);
        self.mobs.spawn_lit(kind, pos, yaw, sky, block)
    }

    /// Atomically spawn a mob only when its complete collision body fits in
    /// loaded, stream-final world state and does not overlap another live solid mob.
    /// This is the programmatic-placement counterpart to [`spawn_mob`]: mods
    /// can create vehicles and other player-placed solid entities without a
    /// racy centre-cell approximation.
    pub fn spawn_mob_checked(&mut self, kind: crate::mob::Mob, pos: Vec3, yaw: f32) -> Option<u64> {
        if !self.mob_spawn_pose_clear(kind, pos, yaw) {
            return None;
        }
        self.spawn_mob(kind, pos, yaw)
    }

    pub(crate) fn mob_spawn_pose_clear(&self, kind: crate::mob::Mob, pos: Vec3, yaw: f32) -> bool {
        let obstacles = self.mobs.solid_obstacles();
        crate::mob::body_pose_fits(
            pos,
            yaw,
            crate::mob::def(kind).size,
            &|x, y, z| self.collision_boxes_at(x, y, z),
            &|x, y, z| self.section_stream_final_at(x, y, z),
            &obstacles,
        )
    }

    pub(crate) fn restore_mobs(&mut self, mobs: impl IntoIterator<Item = SavedMob>) {
        for mob in mobs {
            let (sky, block) = self.mob_render_light_at(mob.pos);
            self.mobs.restore_saved_mob_lit(mob, sky, block);
        }
    }

    fn mob_render_light_at(&self, pos: Vec3) -> (u8, u8) {
        let c = voxel_at(pos + Vec3::new(0.0, 0.3, 0.0));
        let sky = self.skylight6_at_world(c.x, c.y, c.z);
        let block = self.blocklight6_at_world(c.x, c.y, c.z);
        (sky, block)
    }

    /// Record one gameplay noise for the mob AI's hearing batch (see
    /// `mob::noise` for the timing contract). Emitters are the game's own
    /// funnels: player steps, block place/break.
    pub fn push_noise(&mut self, noise: crate::mob::Noise) {
        self.mobs.push_noise(noise);
    }

    /// The riding registry (see `mob::riding`).
    #[inline]
    pub fn riding(&self) -> &crate::mob::riding::Riding {
        &self.riding
    }

    /// Mutable riding registry — the server's riding pass and the engine
    /// safety valves (death, leave) detach through this.
    #[inline]
    pub fn riding_mut(&mut self) -> &mut crate::mob::riding::Riding {
        &mut self.riding
    }

    /// Attach `player` to `seat` of the LIVE mob `mob_id`, validating what the
    /// registry itself cannot: the mob exists and is alive, and the species
    /// row declares that seat. The seat-occupancy and one-mount-per-player
    /// rules live in [`crate::mob::riding::Riding::mount`]. This is the
    /// `MobMount` HostCall's engine seam; the riding pass slaves the player to
    /// the seat starting this same tick.
    pub fn try_mount_player(&mut self, player: u8, mob_id: u64, seat: u8) -> bool {
        let Some(index) = self.mobs.index_of_id(mob_id) else {
            return false;
        };
        let mob = &self.mobs.instances()[index];
        if mob.is_dead() || seat as usize >= crate::mob::def(mob.kind).seats.len() {
            return false;
        }
        self.riding
            .mount(player, crate::mob::riding::MountTarget::Mob(mob_id), seat)
    }

    /// Pin `player` at a static pose anchor — the `PlayerPoseSet` HostCall's
    /// engine seam. The registry enforces one attachment per player and
    /// refuses an exactly-occupied anchor (target equality); WHERE anchors
    /// exist and who takes one is the calling mod's policy. Finite-value
    /// validation happens at the host boundary.
    pub fn try_mount_anchor(&mut self, player: u8, anchor: crate::mob::riding::PoseAnchor) -> bool {
        self.riding
            .mount(player, crate::mob::riding::MountTarget::Anchor(anchor), 0)
    }

    /// Advance the mobs one fixed game tick against an immutable view of the rest of
    /// the world (the field is moved out so the `&mut Mobs` and `&World` borrows stay
    /// disjoint). Returns the gameplay events mobs produced this tick, for `Game` to
    /// apply through the relevant damage pipelines.
    pub fn tick_mobs(
        &mut self,
        dt: f32,
        anchors: &[crate::mob::PlayerAnchor],
    ) -> crate::mob::MobTickEvents {
        // Feed this tick's announced block changes to the confinement cache
        // BEFORE the mobs decide: a pen edit must never leave a mob acting on
        // a stale region.
        let (changed, overflow) = self.take_nav_changes();
        self.mobs.invalidate_confined_regions(&changed, overflow);
        if self.mobs.is_empty() {
            // Nobody is listening: drop the tick's noise batch, or a mob-free
            // world would accumulate the player's footsteps forever.
            self.mobs.discard_noises();
            return crate::mob::MobTickEvents::default();
        }
        let freeze_unloaded = self.save.is_some();
        let mut mobs = std::mem::take(&mut self.mobs);
        let attacks = mobs.tick(dt, self, anchors, freeze_unloaded);
        self.mobs = mobs;
        attacks
    }

    /// Run one natural mob-spawn attempt (the passive backfill trickle; the
    /// caller owns the cadence). Returns the mobs actually spawned, for the
    /// caller to report as `mob_spawned` events.
    pub fn spawn_mobs_tick(&mut self, player_pos: Vec3) -> Vec<(u64, crate::mob::Mob, Vec3)> {
        let mut mobs = std::mem::take(&mut self.mobs);
        let spawned = mobs.spawn_tick(self, player_pos);
        self.mobs = mobs;
        spawned
    }

    /// Run one worldgen-population step around `player_pos` (see `mob::populate`):
    /// place the one-time herds of nearby chunks whose deterministic roll says so,
    /// and record the chunks that spawned in the persisted populated set. Returns
    /// the mobs spawned, for the caller's `mob_spawned` events.
    pub fn populate_mobs_tick(&mut self, player_pos: Vec3) -> Vec<(u64, crate::mob::Mob, Vec3)> {
        let mut mobs = std::mem::take(&mut self.mobs);
        let (spawned, populated) = mobs.populate_tick(self, player_pos);
        self.mobs = mobs;
        self.populated_columns.extend(populated);
        spawned
    }

    /// Whether `chunk`'s one-time worldgen herd already spawned (this session or
    /// any earlier one — the set is restored from `level.dat` at world open).
    pub fn column_populated(&self, chunk: ChunkPos) -> bool {
        self.populated_columns.contains(&chunk)
    }

    /// The persisted populated-chunk set, for the `level.dat` encoder.
    pub fn populated_columns(&self) -> &BTreeSet<ChunkPos> {
        &self.populated_columns
    }

    /// Restore the populated-chunk set at world open (before the first tick, so
    /// the first population pass already sees every historical herd).
    pub fn set_populated_columns(&mut self, set: BTreeSet<ChunkPos>) {
        self.populated_columns = set;
    }
}
