mod camp;
mod daylight;
mod keys;

use mod_sdk::*;
use weather_core::feed::FieldFeed;
use weather_core::FieldParams;

const MONSTERS_TICK_SYSTEM: u32 = 1;
const GEN_CAMPS: u32 = 1;
const MONSTERS_HOSTILE_SPAWNER: u32 = 1;
const ON_MOD_EVENT: u32 = 1;

const TIME_KEY: &str = "petramond:time";

const HUSHJAW_BELOW_Y: i32 = -16;
const HUSHJAW_MIN_PLAYER_DIST: f32 = 32.0;
const HUSHJAW_SPACING: f32 = 32.0;
const HUSHJAW_CLAIM_PER_100: u64 = 10;

const ZOMBIE_CROWD_RADIUS: f32 = 24.0;
const ZOMBIE_CROWD_LIMIT: usize = 4;

const SPAWN_LIGHT_THRESHOLD: f32 = 24.0;
const SUNBURN_RADIUS: f32 = 160.0;
const SUNBURN_SKY_THRESHOLD: f32 = weather_core::DIRECT_SKY_MIN as f32;
const MAX_SKY_LIGHT: f32 = 63.0;
const SUNBURN_CHANCE_PER_100: u64 = 5;
const LIGHT_FIRE_TICKS: u32 = 100;
const DARK_COOL_TICKS: u32 = 60;
const RAIN_COOL_BOOST: f32 = 3.0;
#[derive(Default)]
struct Monsters {
    camps: Option<camp::Camps>,
    burning: Option<Burning>,
    weather: FieldFeed,
    spawn_proof: SpawnProof,
    species: Species,
}

#[derive(Copy, Clone)]
struct Burning {
    id: ConditionId,
    light: u8,
    great: u8,
}

#[derive(Default, Copy, Clone)]
struct Species {
    zombie: Option<MobId>,
    hushjaw: Option<MobId>,
}

impl Mod for Monsters {
    fn init(&mut self) {
        self.camps = camp::Camps::new();
        if self.camps.is_some() {
            register_worldgen_feature(WorldgenStage::Trees, GEN_CAMPS, camp::GEN_FILTER);
        }
        register_tick_system(Stage::Spawning, AttachSide::After, 20, MONSTERS_TICK_SYSTEM);
        register_hostile_spawner(0, MONSTERS_HOSTILE_SPAWNER);
        weather_core::feed::subscribe(ON_MOD_EVENT);
        self.burning = resolve_condition_logged(keys::BURNING).and_then(|info| {
            Some(Burning {
                id: info.id,
                light: info.stage("light")?,
                great: info.stage("great")?,
            })
        });
        self.spawn_proof = SpawnProof::new(blocks_by_tag(keys::SPAWN_PROOF_TAG));
        self.species = Species {
            zombie: resolve_mob_logged(keys::ZOMBIE),
            hushjaw: resolve_mob_logged(keys::HUSHJAW),
        };
        log(&format!(
            "initialized: hostile spawner (zombie + hushjaw) + sunburn, {} spawn-proof surfaces",
            self.spawn_proof.count
        ));
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> GenOutput {
        match (feature_id, self.camps.as_mut()) {
            (GEN_CAMPS, Some(camps)) => camps.generate(ctx),
            _ => GenOutput::default(),
        }
    }

    fn gen_claims(&mut self, feature_id: u32, ctx: &ClaimsCtx) -> GenClaims {
        match (feature_id, self.camps.as_mut()) {
            (GEN_CAMPS, Some(camps)) => camps.claims(ctx),
            _ => GenClaims::default(),
        }
    }

    fn tick_system(&mut self, _system_id: u32) {
        self.weather.tick();
        let Some(daylight) = daylight_factor_from_daynight() else {
            return;
        };
        let field = self.weather_field();
        let anchors: Vec<[f64; 3]> = players().iter().map(|p| p.state.pos).collect();
        let near = mobs_near_any(&anchors, SUNBURN_RADIUS);
        self.tick_fire(daylight, field.as_ref(), &near);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        if handler_id == ON_MOD_EVENT {
            self.weather.observe(payload);
        }
        Outcome::Continue
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

fn site_species(
    candidate: &HostileSpawnCandidate,
    daylight: f32,
    spawn_proof: &SpawnProof,
    species: Species,
    ground: &dyn Fn() -> Option<BlockId>,
    claim_roll: &dyn Fn() -> u64,
    nearby: &dyn Fn([f64; 3], f32) -> Vec<MobSnapshot>,
) -> Option<&'static str> {
    if effective_light(candidate.sky_light, candidate.block_light, daylight)
        >= SPAWN_LIGHT_THRESHOLD
    {
        return None;
    }
    if spawn_proof.refuses(ground) {
        return None;
    }
    if hushjaw_admits(candidate, species, claim_roll, nearby) {
        return Some(keys::HUSHJAW);
    }
    zombie_admits(candidate, species, nearby).then_some(keys::ZOMBIE)
}

fn ground_cell(cell: [i32; 3]) -> [i32; 3] {
    [cell[0], cell[1] - 1, cell[2]]
}

fn zombie_admits(
    candidate: &HostileSpawnCandidate,
    species: Species,
    nearby: &dyn Fn([f64; 3], f32) -> Vec<MobSnapshot>,
) -> bool {
    nearby(candidate.pos, ZOMBIE_CROWD_RADIUS)
        .iter()
        .filter(|m| Some(m.kind) == species.zombie)
        .count()
        < ZOMBIE_CROWD_LIMIT
}

fn hushjaw_admits(
    candidate: &HostileSpawnCandidate,
    species: Species,
    claim_roll: &dyn Fn() -> u64,
    nearby: &dyn Fn([f64; 3], f32) -> Vec<MobSnapshot>,
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

#[derive(Default)]
struct SpawnProof {
    proof: Vec<bool>,
    count: usize,
}

impl SpawnProof {
    fn new(blocks: Vec<BlockId>) -> Self {
        let mut set = Self {
            proof: vec![false; blocks.iter().map(|b| b.0 as usize + 1).max().unwrap_or(0)],
            count: 0,
        };
        for b in blocks {
            if !set.proof[b.0 as usize] {
                set.proof[b.0 as usize] = true;
                set.count += 1;
            }
        }
        set
    }

    /// Whether the floor a body would stand on refuses the spawn. Only reads the world if some pack
    /// actually flagged something.
    ///
    /// An unreadable floor refuses. Core admitted the site from a merely loaded cell, but a mod's
    /// `get_block` is stream-final, so this is exactly the gap when a player first drops into a
    /// fresh cavern. Skipped spawns are cheap (32 attempts/tick, 20 ticks/sec). A monster in a
    /// cavern marked safe by the ground predicate must remain free of monsters.
    fn refuses(&self, ground: &dyn Fn() -> Option<BlockId>) -> bool {
        self.count > 0
            && ground().is_none_or(|b| self.proof.get(b.0 as usize).copied().unwrap_or(false))
    }
}

impl Monsters {
    fn weather_field(&self) -> Option<FieldParams> {
        match world_kv_get(weather_core::CLOCK_KEY).and_then(|b| weather_core::decode_clock(&b)) {
            Some(clock) => self.weather.params_at(clock),
            None => self.weather.params(),
        }
    }

    fn tick_fire(&mut self, daylight: f32, field: Option<&FieldParams>, near: &[MobSnapshot]) {
        let Some(fire) = self.burning else {
            return;
        };
        let sun_can_burn = MAX_SKY_LIGHT * daylight >= SUNBURN_SKY_THRESHOLD;
        let roll = rng_u64("sunburn");
        for mob in near.iter().filter(|m| Some(m.kind) == self.species.zombie) {
            let burning = mob.conditions.iter().find(|c| c.condition == fire.id);
            if burning.is_none() && splitmix64_mix(roll ^ mob.id) % 100 >= SUNBURN_CHANCE_PER_100 {
                continue;
            }
            let rain = rain_at(field, mob.pos);
            if rain == 0.0 && !sun_can_burn {
                continue;
            }
            let entity = EntityRef::Mob(mob.id);
            let cell = cell_of(mob.pos);
            let Some(sky) = sky_light(cell) else {
                continue;
            };
            let rained_on = rain > 0.0 && sky >= SUNBURN_SKY_THRESHOLD;
            if rained_on {
                entity_condition_cool(entity, fire.id, (rain * RAIN_COOL_BOOST) as u32);
            } else if rain == 0.0 && sky * daylight >= SUNBURN_SKY_THRESHOLD {
                let (stage, ticks) = if burning.is_some_and(|b| b.elapsed >= LIGHT_FIRE_TICKS) {
                    (fire.great, DARK_COOL_TICKS * 2)
                } else {
                    (fire.light, DARK_COOL_TICKS)
                };
                entity_condition_apply(entity, fire.id, stage, ticks);
            }
        }
    }
}

fn sky_light(cell: [i32; 3]) -> Option<f32> {
    light_at(cell).map(|l| l.sky as f32)
}

fn rain_at(field: Option<&FieldParams>, pos: [f64; 3]) -> f32 {
    field.map_or(0.0, |p| weather_core::rain(pos[0], pos[2], p))
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
    Some(daylight::daylight(t.rem_euclid(1.0)))
}

register_mod!(Monsters);

#[cfg(test)]
mod tests {
    use super::*;

    const MOSS: BlockId = BlockId(200);
    const STONE: BlockId = BlockId(3);

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

    fn alone(_pos: [f64; 3], _radius: f32) -> Vec<MobSnapshot> {
        Vec::new()
    }

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
        assert_eq!(
            species_over(&proof, Some(STONE)),
            Some(keys::ZOMBIE),
            "the site was otherwise perfect, so the FLOOR is what refused it"
        );
        assert_eq!(
            species_over(&proof, None),
            None,
            "an unreadable floor refuses"
        );
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
    fn spawn_proof_membership_covers_wide_and_unmarked_block_ids() {
        let marked = BlockId(300);
        let proof = SpawnProof::new(vec![marked, marked]);
        assert_eq!(proof.count, 1);
        assert!(proof.refuses(&|| Some(marked)));
        assert!(!proof.refuses(&|| Some(BlockId(u16::MAX))));
        let highest = SpawnProof::new(vec![BlockId(u16::MAX)]);
        assert!(highest.refuses(&|| Some(BlockId(u16::MAX))));
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
            Some(keys::ZOMBIE),
            "with nothing tagged, every previously-good site is still good"
        );
        assert_eq!(
            reads.get(),
            0,
            "and the world is never read — no pack pays for a rule it does not use"
        );
    }
}
