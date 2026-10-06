//! The hostile spawner: which monster, if any, takes a dark spawn site the host offers. Zombies
//! anywhere dark, a few to a crowd; the hushjaw now and then, deep down and away from players.
//! A floor a pack tags `monsters:spawn_proof` refuses every site on it.

use mod_sdk::*;

use crate::day_clock::daylight_now;
use crate::keys;
use crate::routes;

const HUSHJAW_BELOW_Y: i32 = -16;
const HUSHJAW_MIN_PLAYER_DIST: f32 = 32.0;
const HUSHJAW_SPACING: f32 = 32.0;
const HUSHJAW_CLAIM_PER_100: u64 = 10;

const ZOMBIE_CROWD_RADIUS: f32 = 24.0;
const ZOMBIE_CROWD_LIMIT: usize = 4;

const SPAWN_LIGHT_THRESHOLD: f32 = 24.0;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub struct Spawner {
    spawn_proof: SpawnProof,
    species: Species,
}

#[derive(Default, Copy, Clone)]
struct Species {
    zombie: Option<MobId>,
    hushjaw: Option<MobId>,
}

impl Spawner {
    pub fn init() -> Spawner {
        register_hostile_spawner(0, routes::Spawner::Hostiles.id());
        Spawner {
            spawn_proof: SpawnProof::new(blocks_by_tag(keys::SPAWN_PROOF_TAG)),
            species: Species {
                zombie: resolve_mob_logged(keys::ZOMBIE),
                hushjaw: resolve_mob_logged(keys::HUSHJAW),
            },
        }
    }

    /// The zombie species, when its row is loaded.
    pub fn zombie(&self) -> Option<MobId> {
        self.species.zombie
    }

    /// How many surfaces refuse spawns.
    pub fn spawn_proof_surfaces(&self) -> usize {
        self.spawn_proof.count
    }

    /// The species to spawn at `candidate`, if any.
    pub fn candidate(&self, candidate: &HostileSpawnCandidate) -> Option<String> {
        let daylight = daylight_now()?;
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

fn effective_light(sky: u8, block: u8, daylight: f32) -> f32 {
    (block as f32).max(sky as f32 * daylight)
}
