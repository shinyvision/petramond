//! monsters — the hostile-monster mod (zombies + hushjaws), and a
//! MOD-INTEROP consumer: it reads the core `petramond:time` world-KV value and
//! the engine's split light channels to decide when to spawn and burn.
//!
//! What it does, all on the deterministic tick:
//! - **Light-based spawning**: core selects physical hostile-spawn candidates
//!   and asks this mod which species admits each one. Both species accept only
//!   when `max(block_light, sky_light * daylight_factor)` is dark enough.
//!   Daylight comes from `petramond:time`, using the same smooth dawn/dusk curve as
//!   core day/night. Dark caves can spawn monsters during the day; torch/block
//!   light blocks the spawn.
//! - **Hushjaw spawning**: on a dark site, the hushjaw claims the spawn when
//!   the site is deep (feet Y below −16), at least 32 blocks from the nearest
//!   player (the candidate's own `nearest_player_dist` — multiplayer-correct),
//!   at least 32 blocks from every live hushjaw (they hunt alone), and a
//!   seeded 10% claim roll passes — otherwise the site falls through to the
//!   zombie. The hushjaw's BEHAVIOR is all engine brain data on its
//!   `mobs.json` row: `chase_sound` (hears walking, block place/break within
//!   12 blocks, locks through walls, forgets after 40 silent ticks, rarely
//!   hunts a heard zombie), `chase_contact` (anything that BUMPS into it —
//!   player or any mob, sneaking or not — is locked and attacked), `retaliate`
//!   (whoever hits it, it knows — after a 20-tick boil-over), and
//!   `melee_attack`. No head_look: it is blind, and it never visually tracks.
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
//!
//! # World-KV keys
//!
//! - reads `petramond:time` (4-byte LE f32 day fraction) — the sanctioned
//!   interop surface published by core day/night.

use mod_sdk::*;

const MONSTERS_TICK_SYSTEM: u32 = 1;
const MONSTERS_HOSTILE_SPAWNER: u32 = 1;

const ZOMBIE_KEY: &str = "monsters:zombie";
const HUSHJAW_KEY: &str = "monsters:hushjaw";
const TIME_KEY: &str = "petramond:time";

/// Hushjaw spawn rules — a deep-cave apex predator, deliberately never near
/// the surface, the player, or its own kind:
/// - only at feet Y strictly below this level;
const HUSHJAW_BELOW_Y: i32 = -16;
/// - never within this many blocks of the nearest player (core's hostile ring
///   already starts at 25; this pushes the floor out further);
const HUSHJAW_MIN_PLAYER_DIST: f32 = 32.0;
/// - never within this many blocks of another live hushjaw (they hunt alone);
const HUSHJAW_SPACING: f32 = 32.0;
/// - and it claims only this % of otherwise-eligible deep dark sites, so
///   zombies still populate the depths around it and running from one hushjaw
///   rarely means running into another.
const HUSHJAW_CLAIM_PER_100: u64 = 10;

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
struct Monsters {
    /// Burn state per burning zombie (by stable mob id). In-memory only:
    /// a sunlit zombie re-rolls ignition next session.
    burning: std::collections::HashMap<u64, Burn>,
}

impl Mod for Monsters {
    fn init(&mut self) {
        register_tick_system(Stage::Spawning, AttachSide::After, 0, MONSTERS_TICK_SYSTEM);
        register_hostile_spawner(0, MONSTERS_HOSTILE_SPAWNER);
        log("initialized: hostile spawner (zombie + hushjaw) + sunburn");
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

    fn hostile_spawn_candidate(
        &mut self,
        _callback_id: u32,
        candidate: &HostileSpawnCandidate,
    ) -> Option<String> {
        let daylight = daylight_factor_from_daynight()?;
        if effective_light(candidate.sky_light, candidate.block_light, daylight)
            >= SPAWN_LIGHT_THRESHOLD
        {
            return None;
        }
        // The hushjaw gets first claim on the deep dark; everything else that
        // is dark enough is a zombie site (core still enforces species caps on
        // whatever key we return).
        if hushjaw_admits(candidate) {
            return Some(HUSHJAW_KEY.to_owned());
        }
        Some(ZOMBIE_KEY.to_owned())
    }
}

/// The hushjaw's spawn rules on a dark, core-validated candidate site — see
/// the `HUSHJAW_*` constants for the policy. The claim roll draws only after
/// the pure position checks pass, and the (host-crossing) spacing query runs
/// only for claimed sites, so quiet ticks stay cheap and the RNG stream is a
/// deterministic function of the deterministic candidate sequence.
fn hushjaw_admits(candidate: &HostileSpawnCandidate) -> bool {
    if candidate.cell[1] >= HUSHJAW_BELOW_Y {
        return false;
    }
    if candidate.nearest_player_dist < HUSHJAW_MIN_PLAYER_DIST {
        return false;
    }
    if splitmix64_mix(rng_u64("hushjaw_claim")) % 100 >= HUSHJAW_CLAIM_PER_100 {
        return false;
    }
    mobs_in_radius(candidate.pos, HUSHJAW_SPACING)
        .iter()
        .all(|m| m.key != HUSHJAW_KEY)
}

impl Monsters {
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
                if splitmix64_mix(roll ^ mob.id) % 100 < SUNBURN_CHANCE_PER_100
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
    let t = ByteReader::new(&bytes).f32()?;
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

register_mod!(Monsters);
