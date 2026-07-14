//! Beds: sleeping, the bed spawn point, and death respawn — all on the tick.
//!
//! Interacting with a bed (`BlockInteraction::Sleep`) tucks the player in and
//! sets the spawn point in one go; the sleep timer then runs [`SLEEP_TICKS`]
//! fixed ticks and either completes (time skips to next morning) or is
//! cancelled by an app-side wake request (ESC / the "Leave bed" button). The
//! presentation side only draws the dark overlay from [`crate::game::Game::sleep_progress01`]
//! and owns which screen is up; every mutation here happens on the game tick.
//!
//! With multiple players, the morning skip is a CROSS-player decision: it fires
//! only when every alive, non-spectator player is asleep and the longest
//! sleeper finished the timer — see [`ServerGame::resolve_sleep_completion`].
//!
//! Waking (and respawning at a bed) never drops the player inside a wall: the
//! deterministic outward scan in [`find_wake_spot`] picks the closest cell
//! beside the bed whose body space is free of collision boxes over solid
//! footing. Respawn without a (still existing) bed falls back to the same
//! random-surface-near-origin pick a fresh world uses
//! ([`crate::worldgen::spawn::find_spawn`] — deliberately OS-entropy random,
//! like the fresh spawn).

use crate::block::{Block, BlockInteraction};
use crate::mathh::{IVec3, Vec3};
use crate::player::{BedSpawn, MAX_HEALTH, PITCH_LIMIT};
use crate::world::World;

use super::game::ServerGame;
use crate::game::tick::TickEvents;

/// Fixed ticks a full sleep takes — 3 seconds at 20 TPS, matching the overlay
/// fade the presentation draws from the same progress.
pub(crate) const SLEEP_TICKS: u32 = 60;

/// Horizontal Chebyshev reach of the wake-spot scan around the bed cells. Past
/// this the bed is walled in tightly enough that waking ON the bed reads better
/// than teleporting through a wall.
const WAKE_SCAN_RADIUS: i32 = 3;

/// Vertical candidate offsets per column, preference order: same level first,
/// then one step up/down, then two (a bed on a ledge or in a pit).
const WAKE_SCAN_DY: [i32; 5] = [0, 1, -1, 2, -2];

/// The in-flight sleep session: which bed (rotated-footprint base cell) and how
/// many ticks have elapsed.
pub(crate) struct SleepState {
    base: IVec3,
    progress: u32,
}

impl ServerGame {
    /// Right-clicked a bed: set the spawn point beside it (first interaction is
    /// enough — completing the sleep is not required, and a daytime interaction
    /// still sets it) and, at night, start sleeping.
    pub(super) fn start_sleep(&mut self, s: usize, pos: IVec3) {
        let Some((_, base, cells)) = self.world.model_group(pos) else {
            return;
        };
        let spot = find_wake_spot(&self.world, &cells).unwrap_or(bed_top_cell(base));
        self.sessions[s].player.bed_spawn = Some(BedSpawn { bed: base, spot });
        // Sleeping is a night action: a daytime click only (re)sets the spawn.
        if !super::daynight::is_night(&self.world) {
            return;
        }
        // Tuck the player into the bed for the sleep; physics settles them onto
        // the mattress (movement input is off while the sleep screen is up).
        let sess = &mut self.sessions[s];
        sess.player.teleport(group_centre(&cells));
        sess.player.vel = Vec3::ZERO;
        sess.player.pitch = PITCH_LIMIT;
        sess.sleep = Some(SleepState { base, progress: 0 });
        // The camera mirror that used to run here for the local player is
        // presentation: the client applies it after the fixed ticks, keyed on
        // this open request (`Game::tick`), before any presentation read.
        sess.request_open_sleep = true;
    }

    /// The bed base cell session `s` currently sleeps in (a session read — the
    /// client derives the lying body's head yaw from it against the REPLICA's
    /// model group; see `Game::sleep_head_yaw`). `None` while awake.
    pub(crate) fn sleep_bed_base(&self, s: usize) -> Option<IVec3> {
        Some(self.sessions[s].sleep.as_ref()?.base)
    }

    /// While session `s` sleeps, the engine yaw the lying body's head faces:
    /// from the bed's base (foot) cell toward its pillow cell — the server
    /// twin of `Game::sleep_head_yaw`, computed against the AUTHORITATIVE
    /// model group and replicated in `PlayerStateRow::sleep_yaw` so observers
    /// without the bed's section still pose the sleeper right.
    pub(crate) fn sleep_head_yaw(&self, s: usize) -> Option<f32> {
        let base = self.sleep_bed_base(s)?;
        let (_, _, cells) = self.world.model_group(base)?;
        let other = cells.iter().copied().find(|c| *c != base)?;
        let d = other - base;
        Some((d.x as f32).atan2(d.z as f32))
    }

    /// Sleep fade progress in `[0, 1]` while session `s` sleeps (clamped — a
    /// sleeper waiting on other players holds at full). `None` while awake.
    /// Replicated per tick in the session's `SelfState`; the client overlay
    /// reads its `SelfView` mirror.
    pub(crate) fn sleep_progress01(&self, s: usize) -> Option<f32> {
        self.sessions[s]
            .sleep
            .as_ref()
            .map(|st| (st.progress as f32 / SLEEP_TICKS as f32).clamp(0.0, 1.0))
    }

    /// Advance sleeping and consume any latched respawn request, on the tick.
    /// Sleep COMPLETION (the morning skip) is cross-player and resolves once
    /// per tick in [`resolve_sleep_completion`](Self::resolve_sleep_completion).
    pub(crate) fn tick_bed_and_respawn(&mut self, s: usize, events: &mut TickEvents) {
        self.tick_respawn(s, events);
        self.tick_sleep(s, events);
    }

    fn tick_sleep(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        let Some(state) = sess.sleep.as_mut() else {
            sess.wake_requested = false;
            return;
        };
        // Dying in bed ends the sleep without a wake teleport — the death
        // screen (and later the respawn) takes over from here.
        if sess.player.health() == 0 {
            sess.sleep = None;
            events.player(s).sleep_ended = true;
            return;
        }
        if std::mem::take(&mut sess.wake_requested) {
            let base = state.base;
            sess.sleep = None;
            self.wake_at_bed(s, base);
            events.player(s).sleep_ended = true;
            return;
        }
        state.progress += 1;
    }

    /// The night skips to morning only when EVERY alive, non-spectator player
    /// is asleep and the longest sleeper has slept the full [`SLEEP_TICKS`] —
    /// then every sleeper wakes at their own bed. A single awake player blocks
    /// the skip; sleepers just keep lying (ESC leaves the bed). With one player
    /// this matches the old single-player behaviour tick-for-tick.
    pub(crate) fn resolve_sleep_completion(&mut self, events: &mut TickEvents) {
        let everyone_asleep = self.sessions.iter().all(|sess| {
            sess.sleep.is_some() || sess.player.is_spectator() || sess.player.health() == 0
        });
        let any_done = self.sessions.iter().any(|sess| {
            sess.sleep
                .as_ref()
                .is_some_and(|st| st.progress >= SLEEP_TICKS)
        });
        if !everyone_asleep || !any_done {
            return;
        }
        super::daynight::skip_to_morning(&mut self.world);
        for s in 0..self.sessions.len() {
            if let Some(state) = self.sessions[s].sleep.take() {
                self.wake_at_bed(s, state.base);
                events.player(s).sleep_ended = true;
            }
        }
    }

    /// Any damage while asleep interrupts the sleep at once (no time skip):
    /// the player wakes beside the bed to face whatever hit them. Called from
    /// the damage funnel; a lethal hit skips the wake teleport — the death
    /// screen takes over where they lay.
    pub(super) fn interrupt_sleep(&mut self, s: usize, events: &mut TickEvents) {
        let Some(state) = self.sessions[s].sleep.take() else {
            return;
        };
        if self.sessions[s].player.health() > 0 {
            self.wake_at_bed(s, state.base);
        }
        events.player(s).sleep_ended = true;
    }

    fn tick_respawn(&mut self, s: usize, events: &mut TickEvents) {
        if !std::mem::take(&mut self.sessions[s].respawn_requested) {
            return;
        }
        // Respawn is only meaningful for a dead player; a stale request
        // (button mashed as the screen closed) must not teleport the living.
        if self.sessions[s].player.health() > 0 {
            return;
        }
        let target = self.respawn_position(s);
        let player = &mut self.sessions[s].player;
        player.teleport(target);
        player.vel = Vec3::ZERO;
        player.set_health(MAX_HEALTH);
        player.clear_damage_immunity();
        // A fresh life starts clean: lingering status effects die with the body.
        player.clear_effects();
        events.player(s).respawned = true;
    }

    /// Where a respawn lands: beside the (still existing) spawn bed, or a
    /// random dry-land surface column near the origin — the fresh-spawn pick.
    fn respawn_position(&mut self, s: usize) -> Vec3 {
        if let Some(bs) = self.sessions[s].player.bed_spawn {
            if !self.world.chunk_loaded(bs.bed.x >> 4, bs.bed.z >> 4) {
                // The bed's chunk isn't loaded, so it can't be verified (or
                // rescanned) — trust the spot chosen when the spawn was set.
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
            // The bed is gone — the spawn point disappears with it.
            self.sessions[s].player.bed_spawn = None;
        }
        let surface = crate::worldgen::spawn::find_spawn(self.world.seed);
        Vec3::new(
            surface.x as f32 + 0.5,
            (surface.y + 1) as f32,
            surface.z as f32 + 0.5,
        )
    }

    /// Wake beside `base`: the freshest safe spot, or on top of the bed when
    /// the scan finds nothing (the bed is walled in).
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

    /// A bed cell at `pos` was broken (before the group is removed): any
    /// session whose spawn bed it was loses that spawn point.
    pub(crate) fn clear_bed_spawn_at(&mut self, pos: IVec3) {
        let Some(base) = self.world.model_group(pos).map(|(_, base, _)| base) else {
            return;
        };
        for sess in &mut self.sessions {
            if sess.player.bed_spawn.is_some_and(|bs| bs.bed == base) {
                sess.player.bed_spawn = None;
            }
        }
    }

    /// A sleep block broke somewhere without a position-aware hook (a natural
    /// break): re-check that every stored spawn bed still exists.
    pub(super) fn validate_bed_spawn(&mut self) {
        for s in 0..self.sessions.len() {
            let Some(bs) = self.sessions[s].player.bed_spawn else {
                continue;
            };
            if self.world.chunk_loaded(bs.bed.x >> 4, bs.bed.z >> 4) && !bed_at(&self.world, bs.bed)
            {
                self.sessions[s].player.bed_spawn = None;
            }
        }
    }
}

fn bed_at(world: &World, pos: IVec3) -> bool {
    Block::from_id(world.chunk_block(pos.x, pos.y, pos.z)).interaction() == BlockInteraction::Sleep
}

/// Feet position standing centred on top of the bed's base cell — the fallback
/// when no clear spot exists beside it.
fn bed_top_cell(base: IVec3) -> IVec3 {
    IVec3::new(base.x, base.y + 1, base.z)
}

/// Feet position at the centre of cell `c` (feet on the cell's floor).
fn cell_centre(c: IVec3) -> Vec3 {
    Vec3::new(c.x as f32 + 0.5, c.y as f32, c.z as f32 + 0.5)
}

/// Centre of the bed group, slightly above the mattress, for the tuck-in.
fn group_centre(cells: &[IVec3]) -> Vec3 {
    let n = cells.len().max(1) as f32;
    let sum = cells.iter().fold(Vec3::ZERO, |acc, c| {
        acc + Vec3::new(c.x as f32 + 0.5, 0.0, c.z as f32 + 0.5)
    });
    let base_y = cells.iter().map(|c| c.y).min().unwrap_or(0);
    Vec3::new(sum.x / n, base_y as f32 + 0.6, sum.z / n)
}

/// The closest cell beside the bed where the player safely fits: both body
/// cells (feet + head) free of collision boxes, solid footing below, never a
/// bed cell itself. Deterministic: candidates are ordered by horizontal ring
/// distance from the bed cells, then by the fixed [`WAKE_SCAN_DY`] preference,
/// then by coordinate — the same world state always wakes at the same spot.
/// `None` when nothing within [`WAKE_SCAN_RADIUS`] qualifies (or the area
/// isn't loaded).
pub(super) fn find_wake_spot(world: &World, bed_cells: &[IVec3]) -> Option<IVec3> {
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
                        continue; // beside the bed, not on/under/over it
                    }
                    if !ring.contains(&c) {
                        ring.push(c);
                    }
                }
            }
        }
        // Multi-cell beds emit overlapping rings; a fixed order keeps the
        // pick deterministic regardless of footprint iteration order.
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

/// Whether a player standing at the centre of cell `c` fits: the column is
/// loaded (an absent section reads as air and would lie), feet and head cells
/// hold no collision boxes, and the cell below does (solid footing).
fn wake_spot_clear(world: &World, c: IVec3) -> bool {
    if !world.chunk_loaded(c.x >> 4, c.z >> 4) {
        return false;
    }
    world.collision_boxes_at(c.x, c.y, c.z).is_empty()
        && world.collision_boxes_at(c.x, c.y + 1, c.z).is_empty()
        && !world.collision_boxes_at(c.x, c.y - 1, c.z).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};

    /// A loaded, empty chunk at (0,0) with a stone floor at y=63 under a 4×4
    /// pad around (5..9, 5..9), so candidates have footing.
    fn world_with_floor() -> World {
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        for x in 0..16 {
            for z in 0..16 {
                w.set_block_world(x, 63, z, Block::Stone);
            }
        }
        w
    }

    fn place_bed(w: &mut World, base: IVec3) -> Vec<IVec3> {
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
        // Beside the bed: ring distance 1, same level, standing on the floor.
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
        assert!(w.collision_boxes_at(spot.x, spot.y, spot.z).is_empty());
        assert!(!w.collision_boxes_at(spot.x, spot.y - 1, spot.z).is_empty());
    }

    #[test]
    fn wake_spot_skips_obstructed_cells_and_deterministically_repeats() {
        let mut w = world_with_floor();
        let base = IVec3::new(7, 64, 7);
        let cells = place_bed(&mut w, base);
        let first = find_wake_spot(&w, &cells).expect("spot");
        // Wall the first pick off (feet-height block) — the scan must move on.
        w.set_block_world(first.x, first.y, first.z, Block::Stone);
        let second = find_wake_spot(&w, &cells).expect("another spot");
        assert_ne!(first, second, "an obstructed cell is never chosen");
        // Same world state → same answer.
        assert_eq!(find_wake_spot(&w, &cells), Some(second));
    }

    #[test]
    fn walled_in_bed_has_no_wake_spot() {
        let mut w = world_with_floor();
        let base = IVec3::new(7, 64, 7);
        let cells = place_bed(&mut w, base);
        // Seal a generous box around the bed, floor to above head height,
        // except the bed cells themselves.
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
