use crate::player::{BedSpawn, MAX_HEALTH, PITCH_LIMIT};
use crate::world::ServerWorld;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::{Block, BlockTag};

use super::game::ServerGame;
use crate::events::tick::TickEvents;

pub const SLEEP_TICKS: u32 = 60;

const WAKE_SCAN_RADIUS: i32 = 3;

const WAKE_SCAN_DY: [i32; 5] = [0, 1, -1, 2, -2];

pub struct SleepState {
    base: IVec3,
    progress: u32,
}

impl ServerGame {
    pub(super) fn start_sleep(&mut self, s: usize, pos: IVec3) -> bool {
        let Some((_, base, cells)) = self.world.model_group(pos) else {
            return false;
        };
        let mut acted = false;
        if bed_at(&self.world, pos) {
            let spot = find_wake_spot(&self.world, &cells).unwrap_or(bed_top_cell(base));
            self.sessions[s].player.bed_spawn = Some(BedSpawn { bed: base, spot });
            acted = true;
        }
        if !super::daynight::is_night(&self.world) {
            return acted;
        }
        let player_id = self.sessions[s].id.0;
        if self.world.riding().mount_of(player_id).is_some() {
            return acted;
        }
        let sess = &mut self.sessions[s];
        sess.player.teleport(group_centre(&cells));
        sess.player.vel = Vec3::ZERO;
        sess.player.pitch = PITCH_LIMIT;
        sess.sim.sleep = Some(SleepState { base, progress: 0 });
        sess.replication.request_open_sleep = true;
        true
    }

    pub fn sleep_bed_base(&self, s: usize) -> Option<IVec3> {
        Some(self.sessions[s].sim.sleep.as_ref()?.base)
    }

    pub fn sleep_head_yaw(&self, s: usize) -> Option<f32> {
        let base = self.sleep_bed_base(s)?;
        let (_, _, cells) = self.world.model_group(base)?;
        let other = cells.iter().copied().find(|c| *c != base)?;
        let d = other - base;
        Some((d.x as f32).atan2(d.z as f32))
    }

    pub fn sleep_progress01(&self, s: usize) -> Option<f32> {
        self.sessions[s]
            .sim
            .sleep
            .as_ref()
            .map(|st| (st.progress as f32 / SLEEP_TICKS as f32).clamp(0.0, 1.0))
    }

    pub fn tick_bed_and_respawn(&mut self, s: usize, events: &mut TickEvents) {
        self.tick_respawn(s, events);
        self.tick_sleep(s, events);
    }

    fn tick_sleep(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        let Some(state) = sess.sim.sleep.as_mut() else {
            sess.input.wake_requested = false;
            return;
        };
        if sess.player.health() == 0 {
            sess.sim.sleep = None;
            events.player(s).sleep_ended = true;
            return;
        }
        if std::mem::take(&mut sess.input.wake_requested) {
            let base = state.base;
            sess.sim.sleep = None;
            self.wake_at_bed(s, base);
            events.player(s).sleep_ended = true;
            return;
        }
        state.progress += 1;
    }

    pub fn resolve_sleep_completion(&mut self, events: &mut TickEvents) {
        let everyone_asleep = self.sessions.iter().all(|sess| {
            sess.sim.sleep.is_some() || sess.player.is_spectator() || sess.player.health() == 0
        });
        let any_done = self.sessions.iter().any(|sess| {
            sess.sim
                .sleep
                .as_ref()
                .is_some_and(|st| st.progress >= SLEEP_TICKS)
        });
        if !everyone_asleep || !any_done {
            return;
        }
        super::daynight::skip_to_morning(&mut self.world);
        for s in 0..self.sessions.len() {
            if let Some(state) = self.sessions[s].sim.sleep.take() {
                self.wake_at_bed(s, state.base);
                events.player(s).sleep_ended = true;
            }
        }
    }

    pub(super) fn interrupt_sleep(&mut self, s: usize, events: &mut TickEvents) {
        let Some(state) = self.sessions[s].sim.sleep.take() else {
            return;
        };
        if self.sessions[s].player.health() > 0 {
            self.wake_at_bed(s, state.base);
        }
        events.player(s).sleep_ended = true;
    }

    fn tick_respawn(&mut self, s: usize, events: &mut TickEvents) {
        if !std::mem::take(&mut self.sessions[s].input.respawn_requested) {
            return;
        }
        if self.sessions[s].player.health() > 0 {
            return;
        }
        let target = self.respawn_position(s);
        let player = &mut self.sessions[s].player;
        player.teleport(target);
        player.vel = Vec3::ZERO;
        player.set_health(MAX_HEALTH);
        player.clear_damage_immunity();
        player.clear_effects();
        player.clear_exposure();
        events.player(s).respawned = true;
    }

    fn respawn_position(&mut self, s: usize) -> petramond_math::world_pos::WorldPos {
        if let Some(bs) = self.sessions[s].player.bed_spawn {
            if !self.world.data().chunk_loaded(bs.bed.x >> 4, bs.bed.z >> 4) {
                return cell_centre(bs.spot);
            }
            if bed_at(&self.world, bs.bed) {
                if let Some((_, _, cells)) = self.world.model_group(bs.bed) {
                    if let Some(spot) = find_wake_spot(&self.world, &cells) {
                        return cell_centre(spot);
                    }
                }
                return cell_centre(bs.spot);
            }
            self.sessions[s].player.bed_spawn = None;
        }
        let surface = petramond_worldgen::spawn::find_spawn(self.world.data().seed);
        petramond_math::world_pos::WorldPos::block_min(surface) + Vec3::new(0.5, 1.0, 0.5)
    }

    fn wake_at_bed(&mut self, s: usize, base: IVec3) {
        let cells = self
            .world
            .model_group(base)
            .map(|(_, _base, cells)| cells)
            .unwrap_or_else(|| vec![base]);
        let spot = find_wake_spot(&self.world, &cells).unwrap_or(bed_top_cell(base));
        let player = &mut self.sessions[s].player;
        player.teleport(cell_centre(spot));
        player.vel = Vec3::ZERO;
    }

    pub fn clear_bed_spawn_at(&mut self, pos: IVec3) {
        let Some(base) = self.world.model_group(pos).map(|(_, base, _)| base) else {
            return;
        };
        for sess in &mut self.sessions {
            if sess.player.bed_spawn.is_some_and(|bs| bs.bed == base) {
                sess.player.bed_spawn = None;
            }
        }
    }

    pub(super) fn validate_bed_spawn(&mut self) {
        for s in 0..self.sessions.len() {
            let Some(bs) = self.sessions[s].player.bed_spawn else {
                continue;
            };
            if self.world.data().chunk_loaded(bs.bed.x >> 4, bs.bed.z >> 4)
                && !bed_at(&self.world, bs.bed)
            {
                self.sessions[s].player.bed_spawn = None;
            }
        }
    }
}

fn bed_at(world: &ServerWorld, pos: IVec3) -> bool {
    Block::from_id(world.data().chunk_block(pos.x, pos.y, pos.z)).has_tag(BlockTag::BED)
}

fn bed_top_cell(base: IVec3) -> IVec3 {
    IVec3::new(base.x, base.y + 1, base.z)
}

fn cell_centre(c: IVec3) -> petramond_math::world_pos::WorldPos {
    petramond_math::world_pos::WorldPos::block_min(c) + Vec3::new(0.5, 0.0, 0.5)
}

fn group_centre(cells: &[IVec3]) -> petramond_math::world_pos::WorldPos {
    let n = cells.len().max(1) as f64;
    let (sx, sz) = cells.iter().fold((0.0, 0.0), |(x, z), c| {
        (x + f64::from(c.x) + 0.5, z + f64::from(c.z) + 0.5)
    });
    let base_y = cells.iter().map(|c| c.y).min().unwrap_or(0);
    petramond_math::world_pos::WorldPos::new(sx / n, f64::from(base_y) + 0.6, sz / n)
}

pub(super) fn find_wake_spot(world: &ServerWorld, bed_cells: &[IVec3]) -> Option<IVec3> {
    let base_y = bed_cells.iter().map(|c| c.y).min()?;
    for r in 1..=WAKE_SCAN_RADIUS {
        let mut ring: Vec<IVec3> = Vec::new();
        for bed in bed_cells {
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let c = IVec3::new(bed.x + dx, base_y, bed.z + dz);
                    if bed_cells.iter().any(|b| b.x == c.x && b.z == c.z) {
                        continue;
                    }
                    if !ring.contains(&c) {
                        ring.push(c);
                    }
                }
            }
        }
        ring.sort_by_key(|c| (c.x, c.z));
        for dy in WAKE_SCAN_DY {
            for c in &ring {
                let cand = IVec3::new(c.x, base_y + dy, c.z);
                if wake_spot_clear(world, cand) {
                    return Some(cand);
                }
            }
        }
    }
    None
}

fn wake_spot_clear(world: &ServerWorld, c: IVec3) -> bool {
    if !world.data().chunk_loaded(c.x >> 4, c.z >> 4) {
        return false;
    }
    world.data().collision_boxes_at(c.x, c.y, c.z).is_empty()
        && world
            .data()
            .collision_boxes_at(c.x, c.y + 1, c.z)
            .is_empty()
        && !world
            .data()
            .collision_boxes_at(c.x, c.y - 1, c.z)
            .is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world_with_floor() -> ServerWorld {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        for x in 0..16 {
            for z in 0..16 {
                w.set_block_world(x, 63, z, Block::Stone);
            }
        }
        w
    }

    fn place_bed(w: &mut ServerWorld, base: IVec3) -> Vec<IVec3> {
        assert!(w.place_model_block(base, Block::Bed), "bed places");
        let (_, found_base, cells) = w.model_group(base).expect("bed group");
        assert_eq!(found_base, base);
        cells
    }

    #[test]
    fn wake_spot_is_beside_the_bed_on_open_ground() {
        let mut w = world_with_floor();
        let cells = place_bed(&mut w, IVec3::new(7, 64, 7));
        let spot = find_wake_spot(&w, &cells).expect("open ground has a spot");
        assert_eq!(spot.y, 64);
        assert!(
            cells.iter().all(|c| c.x != spot.x || c.z != spot.z),
            "never on the bed's own column: {spot:?}"
        );
        assert!(
            cells
                .iter()
                .any(|c| (c.x - spot.x).abs().max((c.z - spot.z).abs()) == 1),
            "adjacent to a bed cell: {spot:?}"
        );
        assert!(w
            .data()
            .collision_boxes_at(spot.x, spot.y, spot.z)
            .is_empty());
        assert!(!w
            .data()
            .collision_boxes_at(spot.x, spot.y - 1, spot.z)
            .is_empty());
    }

    #[test]
    fn wake_spot_skips_obstructed_cells_and_deterministically_repeats() {
        let mut w = world_with_floor();
        let base = IVec3::new(7, 64, 7);
        let cells = place_bed(&mut w, base);
        let first = find_wake_spot(&w, &cells).expect("spot");
        w.set_block_world(first.x, first.y, first.z, Block::Stone);
        let second = find_wake_spot(&w, &cells).expect("another spot");
        assert_ne!(first, second, "an obstructed cell is never chosen");
        assert_eq!(find_wake_spot(&w, &cells), Some(second));
    }

    #[test]
    fn walled_in_bed_has_no_wake_spot() {
        let mut w = world_with_floor();
        let base = IVec3::new(7, 64, 7);
        let cells = place_bed(&mut w, base);
        for x in base.x - 5..=base.x + 6 {
            for z in base.z - 5..=base.z + 6 {
                for y in 62..=70 {
                    let c = IVec3::new(x, y, z);
                    if cells.contains(&c) {
                        continue;
                    }
                    w.set_block_world(x, y, z, Block::Stone);
                }
            }
        }
        assert_eq!(find_wake_spot(&w, &cells), None);
    }
}
