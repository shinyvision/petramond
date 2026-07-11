//! zombies — the hostile-mob proof-of-concept mod, and a
//! MOD-INTEROP consumer: it reads the core `petramond:time` world-KV value and
//! the engine's split light channels to decide when to spawn and burn.
//!
//! What it does, all on the deterministic tick:
//! - **Light-based spawning**: core selects physical hostile-spawn candidates
//!   and asks this mod whether a zombie admits each one. Zombies accept only
//!   when `max(block_light, sky_light * daylight_factor)` is dark enough.
//!   Daylight comes from `petramond:time`, using the same smooth dawn/dusk curve as
//!   core day/night. Dark caves can spawn zombies during the day; torch/block
//!   light blocks the spawn.
//! - **Sunburn**: every tick, each not-yet-burning zombie in strong direct sky
//!   light has a 5% seeded chance to catch LIGHT fire: the core
//!   `petramond:burn_light` emitter bundle (orange/yellow flames + black smoke
//!   twirling upward) plus 1 `damage_mob` damage every 40 ticks. After 200
//!   ticks of light burn — and only while STILL in direct sunlight — it
//!   escalates to GREAT fire: `petramond:burn_great` (a dense blaze + a faint
//!   orange body tint) and 2 damage every 20 ticks. Out of the sun, the burn
//!   winds down instead: 60 CONSECUTIVE dark ticks demote great fire back to
//!   light, and another 60 put a light burn out entirely (so nightfall or
//!   shade eventually extinguishes every zombie). Damage keeps ticking at the
//!   current stage's cadence while burning, sunlit or not. The burn state
//!   machine is mod state keyed by the stable mob id (in-memory, deliberately
//!   not persisted — a reloaded sunlit zombie simply re-ignites); the visuals
//!   are the engine's keyed mob emitter bundles (`mob_emitter_set`), and the
//!   damage deliberately uses the engine pipeline, so `mob_damage_pre`,
//!   `mob_died`, loot, and the ragdoll all happen. A zombie that burns to
//!   death keeps its flames through the ragdoll, engine-side.
//! - **Sounds**: groan/hurt/death calls are data-driven by the zombie mob row.
//!   The mod does not start audio directly; the engine presentation layer plays
//!   those semantic mob sound hooks.
//! - **I-frames** (the API proof): a `player_damage_pre` handler cancels any
//!   ZOMBIE-sourced damage landing within [`IFRAME_TICKS`] of the previous
//!   zombie hit. A cancel suppresses both the damage AND the knockback
//!   (Phase 3a engine contract). Only `DamageSource::MobAttack { key ==
//!   "zombies:zombie" }` is gated — fall damage, other species, and other
//!   mods' `DamagePlayer` calls pass through untouched: the i-frame window
//!   is a property of zombie melee, not of the player.
//!
//! # World-KV keys
//!
//! - reads `petramond:time` (4-byte LE f32 day fraction) — the sanctioned
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
const TIME_KEY: &str = "petramond:time";
const INVULN_KEY: &str = "zombies:invuln_until";

/// 6-bit effective light strictly below this value allows a spawn. The value
/// is intentionally below ordinary torch light, while still accepting caves
/// with little or no sky/block light.
const SPAWN_LIGHT_THRESHOLD: f32 = 24.0;
/// Sunburn ignition requires strong direct sky light.
const SUNBURN_RADIUS: f32 = 160.0;
const SUNBURN_SKY_THRESHOLD: f32 = 45.0;
/// Per-TICK ignition chance for a sunlit, not-yet-burning zombie.
const SUNBURN_CHANCE_PER_100: u64 = 5;
/// Sunlit ticks on light fire before the burn escalates to great fire.
const LIGHT_FIRE_TICKS: u32 = 200;
/// Consecutive DARK ticks that cool the burn one stage (great → light →
/// out).
const DARK_COOL_TICKS: u32 = 60;
/// Light fire: 1 damage every 40 ticks.
const LIGHT_FIRE_DAMAGE: f32 = 1.0;
const LIGHT_FIRE_DAMAGE_INTERVAL: u32 = 40;
/// Great fire: 2 damage every 20 ticks.
const GREAT_FIRE_DAMAGE: f32 = 2.0;
const GREAT_FIRE_DAMAGE_INTERVAL: u32 = 20;
/// The core emitter bundles this mod attaches (`particle_emitters.json`).
const LIGHT_FIRE_EMITTER: &str = "petramond:burn_light";
const GREAT_FIRE_EMITTER: &str = "petramond:burn_great";
/// 1 s of invulnerability at 20 TPS.
const IFRAME_TICKS: u64 = 20;

#[derive(Copy, Clone, PartialEq)]
enum BurnStage {
    Light,
    Great,
}

/// One zombie's burn: how long it has been in its current stage and how many
/// consecutive ticks it has spent out of direct sunlight.
struct Burn {
    stage: BurnStage,
    stage_ticks: u32,
    dark_ticks: u32,
}

#[derive(Default)]
struct Zombies {
    /// Tick the current i-frame window ends at (authoritative; the world-KV
    /// mirror is for inspection only).
    invuln_until: u64,
    /// Burn state per burning zombie (by stable mob id). In-memory only:
    /// resets on reload like the i-frame window — a sunlit zombie re-rolls
    /// ignition next session.
    burning: std::collections::HashMap<u64, Burn>,
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
        let player = player_state();
        let near = mobs_in_radius(player.pos, SUNBURN_RADIUS);
        self.tick_fire(daylight, &near);
    }

    fn handle_event(&mut self, _handler_id: u32, payload: &mut EventPayload) -> Outcome {
        // Gate ONLY zombie melee (see the module docs for why other damage
        // sources pass through untouched).
        let EventPayload::PlayerDamagePre {
            source: DamageSource::MobAttack { key },
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

impl Zombies {
    /// Ignite sunlit zombies and advance every burning one by one tick.
    fn tick_fire(&mut self, daylight: f32, near: &[MobSnapshot]) {
        // Forget burns whose zombie is gone from the live snapshot: it died
        // (the engine keeps the corpse's flames through the ragdoll on its
        // own) or despawned. One radius covers every live zombie — hostile
        // distance-despawn culls beyond 128, inside SUNBURN_RADIUS.
        self.burning
            .retain(|id, _| near.iter().any(|m| m.id == *id));
        // One host RNG draw per tick; per-zombie rolls derive from it and the
        // stable mob id, so ignition stays deterministic without a host call
        // per zombie per tick.
        let roll = rng_u64("sunburn");
        let mut extinguished: Vec<u64> = Vec::new();
        for mob in near.iter().filter(|m| m.key == ZOMBIE_KEY) {
            let Some(burn) = self.burning.get_mut(&mob.id) else {
                // Not burning. Roll before the light query, so the per-zombie
                // host crossings happen only on the 5% of ticks that might
                // actually ignite.
                if mix(roll ^ mob.id) % 100 < SUNBURN_CHANCE_PER_100
                    && in_sunlight(mob.pos, daylight)
                    && mob_emitter_set(mob.index, LIGHT_FIRE_EMITTER, true)
                {
                    self.burning.insert(
                        mob.id,
                        Burn {
                            stage: BurnStage::Light,
                            stage_ticks: 0,
                            dark_ticks: 0,
                        },
                    );
                }
                continue;
            };

            let sunlit = in_sunlight(mob.pos, daylight);
            burn.stage_ticks += 1;
            burn.dark_ticks = if sunlit { 0 } else { burn.dark_ticks + 1 };

            if burn.dark_ticks >= DARK_COOL_TICKS {
                // Out of the sun long enough: cool one stage.
                burn.dark_ticks = 0;
                match burn.stage {
                    BurnStage::Light => {
                        mob_emitter_set(mob.index, LIGHT_FIRE_EMITTER, false);
                        extinguished.push(mob.id);
                        continue;
                    }
                    BurnStage::Great => {
                        mob_emitter_set(mob.index, GREAT_FIRE_EMITTER, false);
                        mob_emitter_set(mob.index, LIGHT_FIRE_EMITTER, true);
                        burn.stage = BurnStage::Light;
                        burn.stage_ticks = 0;
                    }
                }
            } else if burn.stage == BurnStage::Light
                && sunlit
                && burn.stage_ticks >= LIGHT_FIRE_TICKS
            {
                // 200 ticks of light burn AND still in direct sunlight:
                // escalate. A zombie that found shade before the deadline
                // stays on light fire until the sun catches it again.
                mob_emitter_set(mob.index, LIGHT_FIRE_EMITTER, false);
                mob_emitter_set(mob.index, GREAT_FIRE_EMITTER, true);
                burn.stage = BurnStage::Great;
                burn.stage_ticks = 0;
            }

            // Damage keeps its per-stage cadence while burning, sunlit or not.
            // Stage transitions reset the counter, so the first hit of a stage
            // lands one full interval in.
            let (interval, amount) = match burn.stage {
                BurnStage::Light => (LIGHT_FIRE_DAMAGE_INTERVAL, LIGHT_FIRE_DAMAGE),
                BurnStage::Great => (GREAT_FIRE_DAMAGE_INTERVAL, GREAT_FIRE_DAMAGE),
            };
            if burn.stage_ticks > 0 && burn.stage_ticks % interval == 0 {
                damage_mob(mob.index, amount, None);
            }
        }
        for id in extinguished {
            self.burning.remove(&id);
        }
    }
}

/// SplitMix64 finalizer: spreads one host RNG draw into per-zombie rolls.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
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
