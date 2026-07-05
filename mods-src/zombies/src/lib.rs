//! zombies — the hostile-mob proof-of-concept mod (WIKI/modding.md), and a
//! MOD-INTEROP consumer: it reads the core `llama:time` world-KV value and
//! the engine's split light channels to decide when to spawn and burn.
//!
//! What it does, all on the deterministic tick:
//! - **Light-based spawning**: core selects physical hostile-spawn candidates
//!   and asks this mod whether a zombie admits each one. Zombies accept only
//!   when `max(block_light, sky_light * daylight_factor)` is dark enough.
//!   Daylight comes from `llama:time`, using the same smooth dawn/dusk curve as
//!   core day/night. Dark caves can spawn zombies during the day; torch/block
//!   light blocks the spawn.
//! - **Sunburn**: every second, each zombie in strong direct sky light has a
//!   5% seeded chance to take lethal `hurt_mob` damage. That deliberately uses
//!   the engine death path, so `mob_died`, loot, and the ragdoll all happen.
//! - **Sounds**: groan/hurt/death calls are data-driven by the zombie mob row.
//!   The mod does not start audio directly; the engine presentation layer plays
//!   those semantic mob sound hooks.
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

const ZOMBIE_TICK_SYSTEM: u32 = 1;
const ZOMBIE_HOSTILE_SPAWNER: u32 = 1;
const ON_PLAYER_DAMAGE_PRE: u32 = 1;

const ZOMBIE_KEY: &str = "zombies:zombie";
const TIME_KEY: &str = "llama:time";
const INVULN_KEY: &str = "zombies:invuln_until";

/// 6-bit effective light strictly below this value allows a spawn. The value
/// is intentionally below ordinary torch light, while still accepting caves
/// with little or no sky/block light.
const SPAWN_LIGHT_THRESHOLD: f32 = 24.0;
/// Sunburn checks run once per second and require strong direct sky light.
const SUNBURN_INTERVAL_TICKS: u64 = 20;
const SUNBURN_RADIUS: f32 = 160.0;
const SUNBURN_SKY_THRESHOLD: f32 = 45.0;
const SUNBURN_DAMAGE: f32 = 10_000.0;
const SUNBURN_CHANCE_PER_100: u64 = 5;
/// 1 s of invulnerability at 20 TPS.
const IFRAME_TICKS: u64 = 20;

#[derive(Default)]
struct Zombies {
    /// Tick the current i-frame window ends at (authoritative; the world-KV
    /// mirror is for inspection only).
    invuln_until: u64,
}

impl Mod for Zombies {
    fn init(&mut self) {
        register_tick_system(Stage::Spawning, AttachSide::After, 0, ZOMBIE_TICK_SYSTEM);
        register_hostile_spawner(0, ZOMBIE_HOSTILE_SPAWNER);
        register_event_handler(EventKind::PlayerDamagePre, 0, ON_PLAYER_DAMAGE_PRE);
        log("initialized: hostile spawner + sunburn + zombie-melee i-frames");
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
        if tick % SUNBURN_INTERVAL_TICKS == 0 {
            let player = player_state();
            let near = mobs_in_radius(player.pos, SUNBURN_RADIUS);
            tick_live_zombies(daylight, &near);
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

    fn hostile_spawn_candidate(
        &mut self,
        _callback_id: u32,
        candidate: &HostileSpawnCandidate,
    ) -> Option<String> {
        let daylight = daylight_factor_from_daynight()?;
        (effective_light(candidate.sky_light, candidate.block_light, daylight)
            < SPAWN_LIGHT_THRESHOLD)
            .then(|| ZOMBIE_KEY.to_owned())
    }
}

fn tick_live_zombies(daylight: f32, near: &[MobSnapshot]) {
    for mob in near.iter().filter(|m| m.key == ZOMBIE_KEY) {
        if in_sunlight(mob.pos, daylight) && rng_u64("sunburn") % 100 < SUNBURN_CHANCE_PER_100 {
            let from = [mob.pos[0] + 0.35, mob.pos[1] + 0.4, mob.pos[2] + 0.2];
            hurt_mob(mob.index, SUNBURN_DAMAGE, from);
        }
    }
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

fn effective_light(sky: u8, block: u8, daylight: f32) -> f32 {
    (block as f32).max(sky as f32 * daylight)
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
