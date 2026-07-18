use std::collections::HashMap;

use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::{voxel_at, Vec3};
use crate::mob::{populate, spawn, Instance, Mob, SavedMob};
use crate::world::World;

use super::Mobs;

/// Hard cap on simultaneous mobs, so a spawn loop / debug key can't run the world
/// out of memory. Spawns past this are dropped.
const MAX_MOBS: usize = 256;

impl Mobs {
    /// Spawn a mob of `kind` at `pos` (feet) facing `yaw`. Returns `false` if the
    /// mob cap is reached (the spawn is dropped).
    pub fn spawn(&mut self, kind: Mob, pos: Vec3, yaw: f32) -> bool {
        self.spawn_lit(kind, pos, yaw, 63, 0).is_some()
    }

    /// Spawn a mob with its render light initialized for the first presentation
    /// frame. Use this from world-owned spawn paths where the spawn cell's light
    /// is already available; otherwise a cave spawn can render full-bright until
    /// the next mob tick refreshes cached light. Returns the newborn's stable
    /// session id (the mob's one mod-facing address), or `None` when the mob
    /// cap dropped the spawn.
    pub fn spawn_lit(
        &mut self,
        kind: Mob,
        pos: Vec3,
        yaw: f32,
        skylight: u8,
        blocklight: u8,
    ) -> Option<u64> {
        if self.list.len() >= MAX_MOBS {
            return None;
        }
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut mob = Instance::new(kind, pos, yaw, self.spawn_counter);
        mob.skylight = skylight;
        mob.blocklight = blocklight;
        let id = mob.id();
        self.list.push(mob);
        Some(id)
    }

    /// Remaining room for `kind` under its species and category spawn caps.
    pub fn spawn_room_for(&self, kind: Mob) -> u32 {
        spawn::room_for(&self.list, kind)
    }

    /// Run one natural-spawn step: a single spawn attempt at a random loaded position.
    /// Called once per game tick by `Game`, after [`tick`](Self::tick). Returns the
    /// spawns actually performed (stable id + kind + feet position), for the caller
    /// to report as `mob_spawned` events.
    ///
    /// Mobs that leave the loaded area are no longer dropped here — they are saved into
    /// their chunk as it unloads (see [`take_in_chunk`](Self::take_in_chunk)) and reload
    /// with it. Because the unload harvests them out of the live set, the set still only
    /// holds loaded-area mobs, so the "in the loaded area" caps stay honest — provided
    /// the spawn-relevant area is actually loaded. While saved records within the
    /// nine-chunk census neighborhood are still streaming back in, the attempt holds
    /// off, or every join would refill the caps before those nearby mobs restore.
    pub fn spawn_tick(&mut self, world: &World, player_pos: Vec3) -> Vec<(u64, Mob, Vec3)> {
        // Disjoint borrows: the room test reads the live list, the picker draws `rng`.
        let list = &self.list;
        let chosen = spawn::attempt(world, player_pos, &mut self.rng, |kind| {
            spawn::room_for(list, kind)
        });
        let mut spawned = Vec::new();
        if let Some(spawns) = chosen {
            for s in spawns {
                let c = crate::mathh::voxel_at(s.pos + Vec3::new(0.0, 0.3, 0.0));
                let sky = world.skylight6_at_world(c.x, c.y, c.z);
                let block = world.blocklight6_at_world(c.x, c.y, c.z);
                if let Some(id) = self.spawn_lit(s.kind, s.pos, s.yaw, sky, block) {
                    spawned.push((id, s.kind, s.pos));
                }
            }
        }
        spawned
    }

    /// Run one worldgen-population step around `player_pos` (see [`populate`]):
    /// roll a budgeted batch of nearby unchecked chunks and place their one-time
    /// herds, ignoring the population caps (worldgen stock — only the [`MAX_MOBS`]
    /// memory backstop applies). Returns the spawns performed plus the chunks to
    /// record as populated; the caller owns the persisted populated set, and a
    /// chunk is only recorded once at least one member actually spawned, so a
    /// fully-failed placement retries in a later session.
    pub fn populate_tick(
        &mut self,
        world: &World,
        player_pos: Vec3,
    ) -> (Vec<(u64, Mob, Vec3)>, Vec<ChunkPos>) {
        let herds = populate::attempt(world, player_pos, &mut self.populate_checked);
        let mut spawned = Vec::new();
        let mut populated = Vec::new();
        for herd in herds {
            let mut any = false;
            for s in herd.spawns {
                let c = voxel_at(s.pos + Vec3::new(0.0, 0.3, 0.0));
                let sky = world.skylight6_at_world(c.x, c.y, c.z);
                let block = world.blocklight6_at_world(c.x, c.y, c.z);
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

    /// Drain and return the live mobs resting in section `pos`, as [`SavedMob`]s — used
    /// to bundle them into that section's save record as it unloads. A dead/ragdolling
    /// corpse in the section is removed too, but *not* saved: a corpse is ephemeral (its
    /// loot already dropped when it died), so only living mobs persist.
    pub fn take_in_section(&mut self, pos: SectionPos) -> Vec<SavedMob> {
        let mut taken = Vec::new();
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let c = voxel_at(self.list[i].pos);
            if SectionPos::from_world(c.x, c.y, c.z) == Some(pos) {
                let mob = self.list.swap_remove(i);
                if !mob.is_dead() {
                    taken.push(SavedMob::of(&mob));
                }
            }
        }
        taken
    }

    /// Clone the live mobs grouped by owning section (as [`SavedMob`]s), for the periodic
    /// save flush — the mobs stay active; the clones persist with the section records so a
    /// crash can't lose them. Dead corpses are skipped, as in
    /// [`take_in_section`](Self::take_in_section). A mob outside the world vertical range
    /// (none in normal play) is skipped.
    pub fn saved_by_section(&self) -> HashMap<SectionPos, Vec<SavedMob>> {
        let mut map: HashMap<SectionPos, Vec<SavedMob>> = HashMap::new();
        for m in &self.list {
            if m.is_dead() {
                continue;
            }
            let c = voxel_at(m.pos);
            if let Some(pos) = SectionPos::from_world(c.x, c.y, c.z) {
                map.entry(pos).or_default().push(SavedMob::of(m));
            }
        }
        map
    }

    /// Re-spawn mobs read back from a section's save record now that its section has
    /// loaded. Each gets a fresh AI brain (a reloaded owl simply resumes wandering) and
    /// is subject to the mob cap like any spawn. The saved shear-regrow counter and mod
    /// KV carry over, so a shorn sheep reloads shorn and mod data survives.
    pub fn restore(&mut self, mobs: impl IntoIterator<Item = SavedMob>) {
        for m in mobs {
            self.restore_saved_mob_lit(m, 63, 0);
        }
    }

    pub(crate) fn restore_saved_mob_lit(&mut self, m: SavedMob, skylight: u8, blocklight: u8) {
        if self.spawn_lit(m.kind, m.pos, m.yaw, skylight, blocklight).is_some() {
            if let Some(inst) = self.list.last_mut() {
                inst.set_shear_regrow(m.shear_regrow);
                *inst.mod_kv_mut() = m.kv;
            }
        }
    }

    /// Remove the mob at `index` from the live set immediately — the mod
    /// `DespawnMob` HostCall (no death, no loot, not saved). `swap_remove`, so
    /// it renumbers the last mob into the hole; callers must re-query indices.
    pub fn remove(&mut self, index: usize) -> bool {
        if index < self.list.len() {
            self.list.swap_remove(index);
            true
        } else {
            false
        }
    }
}
