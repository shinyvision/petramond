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
//! - **Water and rain douse the burn** (weather is OPTIONAL): a burning
//!   zombie whose feet cell is water snuffs out instantly, and water blocks
//!   ignition outright. Rain is soft interop: when a weather mod publishes
//!   the `weather:field` world-KV row (see `weather-core`), each zombie
//!   evaluates the field at its own column. Rain reaching the zombie (raw
//!   direct sky, daylight-independent — night rain wets too) cools the burn
//!   like darkness but FASTER, scaling with intensity (a downpour counts
//!   several dark ticks per tick), and any rain-band cloud overhead blocks
//!   ignition and escalation — the same deck that rains is the deck that
//!   occludes the sun, which engine skylight cannot know. No weather mod (or
//!   a stale row whose clock stamp fell behind `petramond:clock` — world KV
//!   persists past an uninstall) means a permanently clear sky: sunburn is
//!   exactly the standalone behavior above.
//! - **Sounds**: groan/hurt/death calls are data-driven by the zombie mob row.
//!   The mod does not start audio directly; the engine presentation layer plays
//!   those semantic mob sound hooks.
//!
//! # World-KV keys
//!
//! - reads `petramond:time` (4-byte LE f32 day fraction) — the sanctioned
//!   interop surface published by core day/night.
//! - reads `weather:field` (a `weather_core::FieldRow`) and `petramond:clock`
//!   (8-byte LE u64, the row's freshness reference) — both OPTIONAL; absent
//!   or stale means "clear sky".

use mod_sdk::*;
use weather_core::FieldParams;

const MONSTERS_TICK_SYSTEM: u32 = 1;
const MONSTERS_HOSTILE_SPAWNER: u32 = 1;

const ZOMBIE_KEY: &str = "monsters:zombie";
const HUSHJAW_KEY: &str = "monsters:hushjaw";
const TIME_KEY: &str = "petramond:time";
const WATER_BLOCK: &str = "petramond:water";

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

/// Zombie crowding rules — the population cap says how many zombies the world
/// holds; these say they may not all hold the same cavern. A candidate site is
/// refused while this many zombies already stand within the radius, so the
/// spawn pressure redistributes across the whole dark volume instead of
/// piling a horde into one room (dark caves are the only admitted surface by
/// day, and the despawn churn kept refilling the same rooms).
/// The radius matches the zombie's `chase_player` radius: within it, every
/// crowd member aggros together, so it is the natural "one encounter" scale.
const ZOMBIE_CROWD_RADIUS: f32 = 24.0;
const ZOMBIE_CROWD_LIMIT: usize = 4;

/// 6-bit effective light strictly below this value allows a spawn. The value
/// is intentionally below ordinary torch light, while still accepting caves
/// with little or no sky/block light.
const SPAWN_LIGHT_THRESHOLD: f32 = 24.0;
/// Sunburn ignition requires strong direct sky light — the shared cross-mod
/// direct-sky threshold (rain lands exactly where the naked sun reaches).
const SUNBURN_RADIUS: f32 = 160.0;
const SUNBURN_SKY_THRESHOLD: f32 = weather_core::DIRECT_SKY_MIN as f32;
/// Per-TICK ignition chance for a sunlit, not-yet-burning zombie.
const SUNBURN_CHANCE_PER_100: u64 = 5;
/// Sunlit ticks on light fire before the burn escalates to great fire.
const LIGHT_FIRE_TICKS: u32 = 200;
/// Consecutive DARK ticks that cool the burn one stage (great → light →
/// out).
const DARK_COOL_TICKS: u32 = 60;
/// Rain cools faster than shade, scaling with how hard it pours: a
/// rained-on tick counts as `1 + rain_intensity * this` dark ticks, so a
/// full downpour winds a stage down in ~15 ticks where shade takes 60,
/// while a drizzle only takes the sun away. Snow douses identically — the
/// field is phase-agnostic here, and smothering a fire is what snow does.
const RAIN_COOL_BOOST: f32 = 3.0;
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
    /// The engine water block, resolved once — a burning zombie standing in
    /// it snuffs out instantly.
    water: Option<BlockId>,
}

impl Mod for Monsters {
    fn init(&mut self) {
        // Priority 20: behind the weather mod's 10 in the same stage window,
        // so the `weather:field` row read each tick is THIS tick's publish.
        register_tick_system(Stage::Spawning, AttachSide::After, 20, MONSTERS_TICK_SYSTEM);
        register_hostile_spawner(0, MONSTERS_HOSTILE_SPAWNER);
        self.water = resolve_block_logged(WATER_BLOCK);
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
        let field = weather_field();
        let player = player_state();
        let near = mobs_in_radius(player.pos, SUNBURN_RADIUS);
        self.tick_fire(daylight, field.as_ref(), &near);
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
        zombie_admits(candidate).then(|| ZOMBIE_KEY.to_owned())
    }
}

/// The zombie's one site rule beyond darkness: the local crowd gate — see the
/// `ZOMBIE_CROWD_*` constants for the policy. The (host-crossing) radius query
/// is the whole check, so it runs only for sites that already passed the light
/// gate and the hushjaw claim.
fn zombie_admits(candidate: &HostileSpawnCandidate) -> bool {
    mobs_in_radius(candidate.pos, ZOMBIE_CROWD_RADIUS)
        .iter()
        .filter(|m| m.key == ZOMBIE_KEY)
        .count()
        < ZOMBIE_CROWD_LIMIT
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
    fn tick_fire(&mut self, daylight: f32, field: Option<&FieldParams>, near: &[MobSnapshot]) {
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
        let water = self.water;
        let mut extinguished: Vec<u64> = Vec::new();
        for mob in near.iter().filter(|m| m.key == ZOMBIE_KEY) {
            let Some(burn) = self.burning.get_mut(&mob.id) else {
                // Not burning. Roll first, then the pure rain check (any
                // rain-band cloud overhead occludes the sun — engine
                // skylight can't know that), so the per-zombie host
                // crossings happen only on the few ticks that might
                // actually ignite. Standing in water blocks ignition too.
                if splitmix64_mix(roll ^ mob.id) % 100 < SUNBURN_CHANCE_PER_100
                    && rain_at(field, mob.pos) == 0.0
                    && in_sunlight(mob.pos, daylight)
                    && !in_water(water, cell_of(mob.pos))
                    && mob_emitter_set(mob.id, LIGHT_FIRE_EMITTER, true)
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

            let cell = cell_of(mob.pos);
            // Dunked: water snuffs the fire outright, whatever the stage.
            if in_water(water, cell) {
                match burn.stage {
                    BurnStage::Light => {
                        mob_emitter_set(mob.id, LIGHT_FIRE_EMITTER, false);
                    }
                    BurnStage::Great => {
                        mob_emitter_set(mob.id, GREAT_FIRE_EMITTER, false);
                    }
                }
                extinguished.push(mob.id);
                continue;
            }

            let sky = sky_light(cell);
            let rain_i = rain_at(field, mob.pos);
            // Rain reaches the zombie only under direct sky (raw sky light,
            // daylight-independent: night rain wets too). When it does, the
            // same deck that rains occludes the sun, so a rained-on zombie
            // is never "sunlit" — its burn only winds down.
            let rained_on = rain_i > 0.0 && sky.is_some_and(|s| s >= SUNBURN_SKY_THRESHOLD);
            let sunlit =
                !rained_on && sky.is_some_and(|s| s * daylight >= SUNBURN_SKY_THRESHOLD);
            burn.stage_ticks += 1;
            burn.dark_ticks = if sunlit {
                0
            } else if rained_on {
                burn.dark_ticks + 1 + (rain_i * RAIN_COOL_BOOST) as u32
            } else {
                burn.dark_ticks + 1
            };

            if burn.dark_ticks >= DARK_COOL_TICKS {
                // Out of the sun (or rained on) long enough: cool one stage.
                burn.dark_ticks = 0;
                match burn.stage {
                    BurnStage::Light => {
                        mob_emitter_set(mob.id, LIGHT_FIRE_EMITTER, false);
                        extinguished.push(mob.id);
                        continue;
                    }
                    BurnStage::Great => {
                        mob_emitter_set(mob.id, GREAT_FIRE_EMITTER, false);
                        mob_emitter_set(mob.id, LIGHT_FIRE_EMITTER, true);
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
                mob_emitter_set(mob.id, LIGHT_FIRE_EMITTER, false);
                mob_emitter_set(mob.id, GREAT_FIRE_EMITTER, true);
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
                // Burn is steady damage-over-time: its pipeline carries the
                // usual flash/sound/death presentation but NO knockback and
                // NO `Immunity` — burn ticks are neither blocked by the
                // engine i-frame window nor grant one, so a burning zombie
                // can still be meleed at full cadence.
                damage_mob_with_feedback(mob.id, amount, None, burn_feedback());
            }
        }
        for id in extinguished {
            self.burning.remove(&id);
        }
    }

}

/// The burn tick's damage pipeline: the default presentation (flash, hurt /
/// death sounds, ragdoll on a lethal tick) minus knockback — fire doesn't
/// shove — and minus `Immunity`, so burn damage neither respects nor grants
/// the engine i-frame window.
fn burn_feedback() -> MobDamageFeedback {
    MobDamageFeedback {
        components: vec![
            MobDamageFeedbackComponent::DecreaseHealth,
            MobDamageFeedbackComponent::Flash { duration: 0.3 },
            MobDamageFeedbackComponent::Sound {
                category: MobDamageSound::Hurt,
            },
            MobDamageFeedbackComponent::Sound {
                category: MobDamageSound::Death,
            },
            MobDamageFeedbackComponent::Ragdoll,
        ],
    }
}

/// The cell holds engine water. `get_block`'s stream-finality gate
/// (`None`) reads as "not water" — never act on frozen state.
fn in_water(water: Option<BlockId>, cell: [i32; 3]) -> bool {
    water.is_some() && get_block(cell) == water
}

fn cell_of(pos: [f32; 3]) -> [i32; 3] {
    [
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ]
}

/// Raw sky light at the cell; `None` while the section is unloaded or its
/// streamed content is not final (`light_at` carries the gate itself).
fn sky_light(cell: [i32; 3]) -> Option<f32> {
    light_at(cell).map(|l| l.sky as f32)
}

fn in_sunlight(pos: [f32; 3], daylight: f32) -> bool {
    sky_light(cell_of(pos)).is_some_and(|sky| sky * daylight >= SUNBURN_SKY_THRESHOLD)
}

/// Rain intensity of the weather field at the mob's column; 0 with no field.
fn rain_at(field: Option<&FieldParams>, pos: [f32; 3]) -> f32 {
    field.map_or(0.0, |p| weather_core::rain(pos[0], pos[2], p))
}

/// The weather mod's published field row, verified FRESH. `None` = clear
/// sky: no weather mod installed, or a stale row a removed weather mod left
/// in the persistent world KV. The stamp is checked against
/// `petramond:clock` when core publishes one; without a shared clock the
/// stamp is unverifiable and the row is trusted (matching the weather mod's
/// own session-tick clock fallback in clockless harnesses).
fn weather_field() -> Option<FieldParams> {
    let row = world_kv_get(weather_core::KV_FIELD)?;
    let clock =
        world_kv_get(weather_core::CLOCK_KEY).and_then(|b| weather_core::decode_clock(&b));
    weather_core::fresh_params(&row, clock)
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
