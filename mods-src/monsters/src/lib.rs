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
//! - **Spawn-proof surfaces**: a dark site is still refused when the block
//!   under the feet carries the `monsters:spawn_proof` block tag. Membership
//!   is pure data — any pack lists the tag on any of its `blocks.json` rows
//!   (the mushroom caverns' moss does, so a grove stays serene until the
//!   player breaks its floor) — and with nothing tagged anywhere this rule
//!   costs nothing and changes nothing.
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

/// Block tag marking a surface no hostile spawns ON. Any pack lists it on any
/// `blocks.json` row and that block is spawn-proof everywhere in the world,
/// with no code change here and none in the engine — which knows neither this
/// tag nor any block that carries it. See [`SpawnProof`].
const SPAWN_PROOF_TAG: &str = "monsters:spawn_proof";

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
    /// Surfaces no hostile spawns on, resolved once from the tag.
    spawn_proof: SpawnProof,
    /// This pack's species ids, resolved once. A mob snapshot names its
    /// species by ID, never by string — a crowd query answers dozens per
    /// tick and a heap string each would be the marshalling's whole cost.
    species: Species,
}

/// The species ids this pack reasons about, resolved once at init.
#[derive(Default, Copy, Clone)]
struct Species {
    zombie: Option<MobId>,
    hushjaw: Option<MobId>,
}

impl Mod for Monsters {
    fn init(&mut self) {
        // Priority 20: behind the weather mod's 10 in the same stage window,
        // so the `weather:field` row read each tick is THIS tick's publish.
        register_tick_system(Stage::Spawning, AttachSide::After, 20, MONSTERS_TICK_SYSTEM);
        register_hostile_spawner(0, MONSTERS_HOSTILE_SPAWNER);
        self.water = resolve_block_logged(WATER_BLOCK);
        // ONE crossing for the whole membership, at init — the house pattern
        // for tag-driven policy. The count is logged because a tag name is a
        // string agreed across two packs: a typo on either side is not a load
        // error anywhere, and this number is the only place it shows up.
        self.spawn_proof = SpawnProof::new(blocks_by_tag(SPAWN_PROOF_TAG));
        self.species = Species {
            zombie: resolve_mob_logged(ZOMBIE_KEY),
            hushjaw: resolve_mob_logged(HUSHJAW_KEY),
        };
        log(&format!(
            "initialized: hostile spawner (zombie + hushjaw) + sunburn, {} spawn-proof surfaces",
            self.spawn_proof.count
        ));
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
        site_species(
            candidate,
            daylight,
            &self.spawn_proof,
            self.species,
            &|| get_block(ground_cell(candidate.cell)),
            &|| splitmix64_mix(rng_u64("hushjaw_claim")),
            &|pos, radius| mobs_in_radius(pos, radius),
        )
        .map(str::to_owned)
    }
}

/// The whole site-admission policy, as a PURE function of what the host said.
/// The host-crossing inputs arrive as closures because the ORDER of the gates
/// is itself policy: the light check is free, the spawn-proof floor costs at
/// most one block read, and each crowd query walks a radius — so a lit site
/// pays nothing, a mossy one pays one read, and only a site that could really
/// hold a monster pays for the crowd walks. Evaluating any of them eagerly
/// would pay for gates the site never reaches. (Being pure is also what makes
/// it testable: host functions are `unreachable!()` off wasm.)
fn site_species(
    candidate: &HostileSpawnCandidate,
    daylight: f32,
    spawn_proof: &SpawnProof,
    species: Species,
    ground: &dyn Fn() -> Option<BlockId>,
    claim_roll: &dyn Fn() -> u64,
    nearby: &dyn Fn([f32; 3], f32) -> Vec<MobSnapshot>,
) -> Option<&'static str> {
    if effective_light(candidate.sky_light, candidate.block_light, daylight)
        >= SPAWN_LIGHT_THRESHOLD
    {
        return None;
    }
    if spawn_proof.refuses(ground) {
        return None;
    }
    // The hushjaw gets first claim on the deep dark; everything else that
    // is dark enough is a zombie site (core still enforces species caps on
    // whatever key we return).
    if hushjaw_admits(candidate, species, claim_roll, nearby) {
        return Some(HUSHJAW_KEY);
    }
    zombie_admits(candidate, species, nearby).then_some(ZOMBIE_KEY)
}

/// The cell a body standing at `cell` has under its feet.
fn ground_cell(cell: [i32; 3]) -> [i32; 3] {
    [cell[0], cell[1] - 1, cell[2]]
}

/// The zombie's one site rule beyond darkness: the local crowd gate — see the
/// `ZOMBIE_CROWD_*` constants for the policy. The (host-crossing) radius query
/// is the whole check, so it runs only for sites that already passed the light
/// gate and the hushjaw claim.
fn zombie_admits(
    candidate: &HostileSpawnCandidate,
    species: Species,
    nearby: &dyn Fn([f32; 3], f32) -> Vec<MobSnapshot>,
) -> bool {
    nearby(candidate.pos, ZOMBIE_CROWD_RADIUS)
        .iter()
        .filter(|m| Some(m.kind) == species.zombie)
        .count()
        < ZOMBIE_CROWD_LIMIT
}

/// The hushjaw's spawn rules on a dark, core-validated candidate site — see
/// the `HUSHJAW_*` constants for the policy. The claim roll draws only after
/// the pure position checks pass, and the (host-crossing) spacing query runs
/// only for claimed sites, so quiet ticks stay cheap and the RNG stream is a
/// deterministic function of the deterministic candidate sequence.
fn hushjaw_admits(
    candidate: &HostileSpawnCandidate,
    species: Species,
    claim_roll: &dyn Fn() -> u64,
    nearby: &dyn Fn([f32; 3], f32) -> Vec<MobSnapshot>,
) -> bool {
    if candidate.cell[1] >= HUSHJAW_BELOW_Y {
        return false;
    }
    if candidate.nearest_player_dist < HUSHJAW_MIN_PLAYER_DIST {
        return false;
    }
    if claim_roll() % 100 >= HUSHJAW_CLAIM_PER_100 {
        return false;
    }
    nearby(candidate.pos, HUSHJAW_SPACING)
        .iter()
        .all(|m| Some(m.kind) != species.hushjaw)
}

/// The surfaces a hostile refuses to spawn ON — the block under the feet —
/// as a dense per-block-id table.
///
/// Membership is data: ANY pack marks ANY block spawn-proof by listing
/// [`SPAWN_PROOF_TAG`] on its `blocks.json` row. No block and no pack is named
/// here, and with nothing tagged the table is empty and this mod behaves
/// exactly as it did before the rule existed.
///
/// A `BlockId` is a u8, so the whole id space is 256 bytes — the dense-table
/// shape the engine uses for its own per-id lookups. The lookup is not the
/// cost on this path (the block read that feeds it crosses the host boundary),
/// but a constant-time table means the cost cannot grow as packs tag more
/// surfaces. Built once in `init` from the tag reply; there is no registry
/// lazy in the guest for it to deadlock against.
struct SpawnProof {
    proof: [bool; 256],
    /// How many surfaces are marked — logged at init as the only signal that
    /// the tag string actually matched something.
    count: usize,
}

impl Default for SpawnProof {
    fn default() -> Self {
        Self {
            proof: [false; 256],
            count: 0,
        }
    }
}

impl SpawnProof {
    fn new(blocks: Vec<BlockId>) -> Self {
        let mut set = Self::default();
        for b in blocks {
            if !set.proof[b.0 as usize] {
                set.proof[b.0 as usize] = true;
                set.count += 1;
            }
        }
        set
    }

    /// Whether the floor a body would stand on refuses the spawn. Reads the
    /// world only when some pack actually marked something.
    ///
    /// An UNREADABLE floor refuses. This deliberately inverts the `in_water`
    /// convention below: core admitted the site from a merely LOADED cell
    /// while a mod's `get_block` is stream-final, so the gap is exactly the
    /// moment a player first descends into a fresh cavern — the most visible
    /// moment there is. A skipped spawn costs nothing (32 attempts a tick, 20
    /// ticks a second); a monster in a cavern that promised safety is the bug
    /// this rule exists to prevent.
    fn refuses(&self, ground: &dyn Fn() -> Option<BlockId>) -> bool {
        self.count > 0 && ground().is_none_or(|b| self.proof[b.0 as usize])
    }
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
        let zombie = self.species.zombie;
        for mob in near.iter().filter(|m| Some(m.kind) == zombie) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two block ids that mean nothing to this mod beyond the tag reply — the
    /// point of the seam is that it never learns which is which.
    const MOSS: BlockId = BlockId(200);
    const STONE: BlockId = BlockId(3);

    /// A site core already validated as physically spawnable: pitch dark, far
    /// from the player, and shallow enough that the hushjaw never claims it,
    /// so the only thing left to vary is the floor.
    fn dark_site() -> HostileSpawnCandidate {
        HostileSpawnCandidate {
            pos: [8.5, 20.0, 8.5],
            cell: [8, 20, 8],
            combined_light: 0,
            sky_light: 0,
            block_light: 0,
            nearest_player_dist: 40.0,
        }
    }

    /// An empty neighbourhood: no crowd, no rival hushjaw.
    fn alone(_pos: [f32; 3], _radius: f32) -> Vec<MobSnapshot> {
        Vec::new()
    }

    /// The hushjaw's claim roll, losing.
    fn no_claim() -> u64 {
        HUSHJAW_CLAIM_PER_100
    }

    fn species_over(proof: &SpawnProof, ground: Option<BlockId>) -> Option<&'static str> {
        site_species(
            &dark_site(),
            1.0,
            proof,
            Species::default(),
            &|| ground,
            &no_claim,
            &alone,
        )
    }

    #[test]
    fn a_spawn_proof_floor_refuses_a_site_that_is_otherwise_perfect() {
        let proof = SpawnProof::new(vec![MOSS]);
        assert_eq!(species_over(&proof, Some(MOSS)), None, "moss refuses");
        // The same site with the moss broken away: this is the gameplay —
        // mine the floor of a serene cavern and it becomes dangerous.
        assert_eq!(
            species_over(&proof, Some(STONE)),
            Some(ZOMBIE_KEY),
            "the site was otherwise perfect, so the FLOOR is what refused it"
        );
        // Stream-finality fail-safe. The most likely thing a later
        // simplification gets backwards, and it leaks spawns exactly when a
        // player first walks into a freshly streamed cavern.
        assert_eq!(
            species_over(&proof, None),
            None,
            "an unreadable floor refuses"
        );
        // The pre-existing gates still fire, in front of this one.
        let lit = HostileSpawnCandidate {
            block_light: 30,
            ..dark_site()
        };
        let stone = || Some(STONE);
        assert_eq!(
            site_species(
                &lit,
                1.0,
                &proof,
                Species::default(),
                &stone,
                &no_claim,
                &alone
            ),
            None,
            "a lit site is still refused"
        );
    }

    #[test]
    fn an_unmarked_world_neither_changes_behaviour_nor_reads_the_floor() {
        let reads = std::cell::Cell::new(0u32);
        let ground = || {
            reads.set(reads.get() + 1);
            Some(MOSS)
        };
        let none = SpawnProof::default();
        assert_eq!(
            site_species(
                &dark_site(),
                1.0,
                &none,
                Species::default(),
                &ground,
                &no_claim,
                &alone
            ),
            Some(ZOMBIE_KEY),
            "with nothing tagged, every previously-good site is still good"
        );
        assert_eq!(
            reads.get(),
            0,
            "and the world is never read — no pack pays for a rule it does not use"
        );
    }
}
