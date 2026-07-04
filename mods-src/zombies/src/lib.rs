//! zombies — the hostile-mob proof-of-concept mod (WIKI/modding.md), and a
//! MOD-INTEROP consumer: it reads the core `llama:time` world-KV value and
//! the engine's split light channels to decide when to spawn and burn.
//!
//! What it does, all on the deterministic tick:
//! - **Light-based spawning** (tick system at `After(Spawning)`): every
//!   [`SPAWN_INTERVAL_TICKS`] it rolls one bounded spawn pass — ring positions
//!   32–128 blocks from the player from the mod's seeded RNG streams, dropped
//!   to the ground by a column scan — then admits one at most, only when
//!   `max(block_light, sky_light * daylight_factor)` is dark enough. Daylight
//!   comes from `llama:time`, using the same smooth dawn/dusk curve as core
//!   day/night. Dark caves can spawn zombies during the day; torch/block
//!   light blocks the spawn.
//! - **Sunburn**: every second, each zombie in strong direct sky light has a
//!   5% seeded chance to take lethal `hurt_mob` damage. That deliberately uses
//!   the engine death path, so `mob_died`, loot, and the ragdoll all happen.
//! - **Groans**: each zombie stores its next groan tick in mob KV. When due,
//!   the mod starts `zombies:groan` with `sound_play_on_mob` using the
//!   snapshot's stable mob id, never its tick-local index.
//! - **I-frames** (the API proof): a `player_damage_pre` handler cancels any
//!   ZOMBIE-sourced damage landing within [`IFRAME_TICKS`] of the previous
//!   zombie hit. A cancel suppresses both the damage AND the knockback
//!   (Phase 3a engine contract). Only `DamageSource::Mob { key ==
//!   "zombies:zombie" }` is gated — fall damage, other species, and other
//!   mods' `DamagePlayer` calls pass through untouched: the i-frame window
//!   is a property of zombie melee, not of the player.
//!
//! # World-KV keys
//!
//! - reads `llama:time` (4-byte LE f32 day fraction) — the sanctioned
//!   interop surface published by core day/night.
//! - writes `zombies:invuln_until` — 8 bytes, little-endian u64: the game
//!   tick the current i-frame window ends at, mirrored for inspection.
//!   The in-memory copy is authoritative within a session and deliberately
//!   resets on reload (worst case: one free hit — trivia, not state worth
//!   persisting).

use mod_sdk::*;

const SPAWN_SYSTEM: u32 = 1;
const ON_PLAYER_DAMAGE_PRE: u32 = 1;

const ZOMBIE_KEY: &str = "zombies:zombie";
const GROAN_SOUND: &str = "zombies:groan";
const TIME_KEY: &str = "llama:time";
const INVULN_KEY: &str = "zombies:invuln_until";
const NEXT_GROAN_KEY: &str = "zombies:next_groan";

/// One spawn roll every tick.
const SPAWN_INTERVAL_TICKS: u64 = 1;
/// Each roll still spawns at most one zombie. Retrying candidate columns keeps
/// sparse cave tunnels from depending on one lucky x/z sample every two seconds.
const SPAWN_ATTEMPTS_PER_ROLL: u32 = 32;
/// Spawn ring around the player, in blocks.
const MIN_SPAWN_DIST: f32 = 25.0;
const MAX_SPAWN_DIST: f32 = 128.0;
/// Live-zombie cap; counted within [`COUNT_RADIUS`] of the player, which
/// covers the 128-block spawn ring plus the 32-block despawn slack.
const MAX_ZOMBIES: usize = 8;
const COUNT_RADIUS: f32 = 160.0;
/// 6-bit effective light strictly below this value allows a spawn. The value
/// is intentionally below ordinary torch light, while still accepting caves
/// with little or no sky/block light.
const SPAWN_LIGHT_THRESHOLD: f32 = 24.0;
/// Sunburn checks run once per second and require strong direct sky light.
const SUNBURN_INTERVAL_TICKS: u64 = 20;
const SUNBURN_SKY_THRESHOLD: f32 = 45.0;
const SUNBURN_DAMAGE: f32 = 10_000.0;
const SUNBURN_CHANCE_PER_100: u64 = 5;
/// Zombies groan every 6–16 seconds, independently per live mob.
const GROAN_MIN_TICKS: u64 = 120;
const GROAN_MAX_TICKS: u64 = 320;
/// 1 s of invulnerability at 20 TPS.
const IFRAME_TICKS: u64 = 20;
/// Ground-scan window around the player's feet Y (spawns follow the player's
/// terrain level; a cliff or pit further off simply fails the roll).
const SCAN_UP: i32 = 8;
const SCAN_DOWN: i32 = 24;

#[derive(Default)]
struct Zombies {
    /// Tick the current i-frame window ends at (authoritative; the world-KV
    /// mirror is for inspection only).
    invuln_until: u64,
}

impl Mod for Zombies {
    fn init(&mut self) {
        register_tick_system(Stage::Spawning, AttachSide::After, 0, SPAWN_SYSTEM);
        register_event_handler(EventKind::PlayerDamagePre, 0, ON_PLAYER_DAMAGE_PRE);
        log("initialized: light spawner + sunburn + groans + zombie-melee i-frames");
    }

    fn tick_system(&mut self, _system_id: u32) {
        // Core day/night publishes this before mods run in a real Game. Absent
        // or malformed time disables the environment-dependent systems for
        // this tick, so the mod remains usable in host-only tests and custom
        // harnesses that choose not to provide a clock.
        let Some(daylight) = daylight_factor_from_daynight() else {
            return;
        };
        let tick = current_tick();
        let player = player_state();
        let near = mobs_in_radius(player.pos, COUNT_RADIUS);

        tick_live_zombies(tick, daylight, &near);
        if tick % SPAWN_INTERVAL_TICKS == 0 {
            self.try_spawn(&player, daylight, &near);
        }
    }

    fn handle_event(&mut self, _handler_id: u32, payload: &mut EventPayload) -> Outcome {
        // Gate ONLY zombie melee (see the module docs for why other damage
        // sources pass through untouched).
        let EventPayload::PlayerDamagePre {
            source: DamageSource::Mob { key },
            ..
        } = payload
        else {
            return Outcome::Continue;
        };
        if key != ZOMBIE_KEY {
            return Outcome::Continue;
        }
        let now = current_tick();
        if now < self.invuln_until {
            // Cancel = the engine drops the damage AND the knockback.
            return Outcome::Cancel;
        }
        self.invuln_until = now + IFRAME_TICKS;
        world_kv_set(INVULN_KEY, self.invuln_until.to_le_bytes().to_vec());
        Outcome::Continue
    }
}

impl Zombies {
    /// One spawn pass: bounded ring positions from the mod's deterministic RNG
    /// streams, each dropped to the ground then light-gated. The pass spawns at
    /// most one zombie; cap reached, unloaded cells, bright cells, and no
    /// standable ground simply advance to the next candidate.
    fn try_spawn(&mut self, player: &PlayerSnapshot, daylight: f32, near: &[MobSnapshot]) {
        if near.iter().filter(|m| m.key == ZOMBIE_KEY).count() >= MAX_ZOMBIES {
            return;
        }

        for _ in 0..SPAWN_ATTEMPTS_PER_ROLL {
            let Some((pos, yaw)) = spawn_candidate(player, daylight) else {
                continue;
            };
            spawn_mob(ZOMBIE_KEY, pos, yaw);
            return;
        }
    }
}

fn spawn_candidate(player: &PlayerSnapshot, daylight: f32) -> Option<([f32; 3], f32)> {
    let angle = (rng_u64("spawn_angle") % 6_283) as f32 / 1_000.0; // ~[0, 2π)
    let dist = MIN_SPAWN_DIST
        + (rng_u64("spawn_dist") % 1_000) as f32 / 1_000.0 * (MAX_SPAWN_DIST - MIN_SPAWN_DIST);
    let (sin, cos) = angle.sin_cos();
    let wx = (player.pos[0] + dist * cos).floor() as i32;
    let wz = (player.pos[2] + dist * sin).floor() as i32;
    let feet_y = ground_y(wx, player.pos[1].floor() as i32, wz)?;
    if !spawn_light_allows([wx, feet_y, wz], daylight) {
        return None;
    }

    let pos = [wx as f32 + 0.5, feet_y as f32, wz as f32 + 0.5];
    let (dx, dz) = (player.pos[0] - pos[0], player.pos[2] - pos[2]);
    Some((pos, (-dx).atan2(-dz)))
}

fn tick_live_zombies(tick: u64, daylight: f32, near: &[MobSnapshot]) {
    let burn_due = tick % SUNBURN_INTERVAL_TICKS == 0;
    for mob in near.iter().filter(|m| m.key == ZOMBIE_KEY) {
        maybe_groan(tick, mob);
        if burn_due
            && in_sunlight(mob.pos, daylight)
            && rng_u64("sunburn") % 100 < SUNBURN_CHANCE_PER_100
        {
            let from = [mob.pos[0] + 0.35, mob.pos[1] + 0.4, mob.pos[2] + 0.2];
            hurt_mob(mob.index, SUNBURN_DAMAGE, from);
        }
    }
}

fn maybe_groan(tick: u64, mob: &MobSnapshot) {
    match mob_next_groan(mob.index) {
        Some(next) if tick >= next => {
            let pitch = 0.95 + (rng_u64("groan_pitch") % 101) as f32 / 1_000.0;
            sound_play_on_mob(mob.id, GROAN_SOUND, 0.65, pitch);
            set_next_groan(mob.index, tick);
        }
        Some(_) => {}
        None => set_next_groan(mob.index, tick),
    }
}

fn mob_next_groan(index: u32) -> Option<u64> {
    let bytes = mob_kv_get(index, NEXT_GROAN_KEY)?;
    let raw: [u8; 8] = bytes.as_slice().try_into().ok()?;
    Some(u64::from_le_bytes(raw))
}

fn set_next_groan(index: u32, tick: u64) {
    let span = GROAN_MAX_TICKS - GROAN_MIN_TICKS + 1;
    let delay = GROAN_MIN_TICKS + rng_u64("groan_delay") % span;
    let _ = mob_kv_set(
        index,
        NEXT_GROAN_KEY,
        tick.saturating_add(delay).to_le_bytes().to_vec(),
    );
}

/// Feet Y of the highest standable cell in column `(wx, wz)` near `py`:
/// full spawn-support below and two air cells of headroom, scanned top-down.
/// The support predicate is host-owned so water, leaves, stairs, and future
/// partial blocks follow the engine's spawn-footing rules.
fn ground_y(wx: i32, py: i32, wz: i32) -> Option<i32> {
    if !is_loaded([wx, py, wz]) {
        return None;
    }
    let ys: Vec<i32> = (py - SCAN_DOWN..=py + SCAN_UP + 1).collect();
    let column = get_blocks(ys.iter().map(|&y| [wx, y, wz]).collect());
    // Index i holds the block at ys[i]; find air-over-solid with air above.
    for i in (1..column.len() - 1).rev() {
        let clear = column[i] == Some(BlockId::AIR) && column[i + 1] == Some(BlockId::AIR);
        if clear && block_is_full_spawn_support([wx, ys[i - 1], wz]) {
            return Some(ys[i]);
        }
    }
    None
}

fn spawn_light_allows(cell: [i32; 3], daylight: f32) -> bool {
    effective_light_at(cell, daylight).is_some_and(|light| light < SPAWN_LIGHT_THRESHOLD)
}

fn in_sunlight(pos: [f32; 3], daylight: f32) -> bool {
    let cell = [
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ];
    if !is_loaded(cell) {
        return false;
    }
    let (_, sky, _) = light_at(cell);
    sky as f32 * daylight >= SUNBURN_SKY_THRESHOLD
}

fn effective_light_at(cell: [i32; 3], daylight: f32) -> Option<f32> {
    if !is_loaded(cell) {
        return None;
    }
    let (_, sky, block) = light_at(cell);
    Some((block as f32).max(sky as f32 * daylight))
}

fn daylight_factor_from_daynight() -> Option<f32> {
    let bytes = world_kv_get(TIME_KEY)?;
    let raw: [u8; 4] = bytes.as_slice().try_into().ok()?;
    let t = f32::from_le_bytes(raw);
    if !t.is_finite() || !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some(daylight(t.rem_euclid(1.0)))
}

fn daylight(t: f32) -> f32 {
    let h = (core::f32::consts::PI * 0.04).sin();
    smoothstep(-h, h, (core::f32::consts::TAU * t).sin())
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

register_mod!(Zombies);
