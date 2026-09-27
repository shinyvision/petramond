use rustc_hash::FxHashSet;

use crate::world::ServerWorld;
use petramond_math::math::IVec3;
use petramond_world::chunk::{ChunkPos, CHUNK_SX, CHUNK_SZ};

use super::spawn::{
    biome_chance_passes, mob_census_ready, nearby_spawn, site_for, spawn_with, species_enabled,
    splitmix, Spawn,
};
use super::{def, defs, Mob, MobCategory, MobRng};

/// Chance that a chunk column's raw roll passes. Drawn FIRST and
/// unconditionally from the chunk's positional stream, so the yes/no is
/// identical every session. The EFFECTIVE herd density is set by
/// [`HERD_SPACING_CHUNKS`] suppression on top of this (~4% of chunks); the raw
/// chance mostly stops mattering once it saturates the spacing grid.
const POPULATE_CHANCE: f32 = 0.10;
/// Minimum Chebyshev chunk distance kept between two herd chunks: a chunk whose
/// draw passes is still suppressed when any chunk within this radius draws a
/// passing, STRONGER roll (lower draw wins; coords break exact ties). The same
/// deterministic symmetric-suppression trick as tree spacing — independent
/// per-chunk chance alone Poisson-clumps, and adjacent 2–5-animal herds read
/// as "sheep everywhere". Purely positional on purpose: a suppressor needs no
/// valid terrain, so coastlines populate conservatively rather than doubly.
const HERD_SPACING_CHUNKS: i32 = 2;
const POPULATE_CHUNK_RADIUS: i32 = 8;
const ROLL_BUDGET_PER_TICK: u32 = 8;
const SITE_TRIES: u32 = 8;
const POPULATE_SALT: u64 = 0x0F0F_5EED_4E7D_0001;

pub(super) struct HerdSpawn {
    pub chunk: ChunkPos,
    pub spawns: Vec<Spawn>,
}

fn chunk_rng(seed: u32, chunk: ChunkPos) -> MobRng {
    let mixed = splitmix(
        (seed as u64)
            ^ POPULATE_SALT
            ^ (chunk.cx as i64 as u64).wrapping_mul(0x632B_E599_37D5_ACE5)
            ^ (chunk.cz as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
    );
    MobRng::new(mixed)
}

fn herd_draw(seed: u32, chunk: ChunkPos) -> Option<u32> {
    let d = chunk_rng(seed, chunk).next_f32();
    (d < POPULATE_CHANCE).then(|| d.to_bits())
}

fn wins_spacing(seed: u32, chunk: ChunkPos, own: u32) -> bool {
    for dz in -HERD_SPACING_CHUNKS..=HERD_SPACING_CHUNKS {
        for dx in -HERD_SPACING_CHUNKS..=HERD_SPACING_CHUNKS {
            if dx == 0 && dz == 0 {
                continue;
            }
            let n = ChunkPos::new(chunk.cx + dx, chunk.cz + dz);
            let Some(theirs) = herd_draw(seed, n) else {
                continue;
            };
            if (theirs, n.cx, n.cz) < (own, chunk.cx, chunk.cz) {
                return false;
            }
        }
    }
    true
}

pub(super) fn attempt(
    world: &ServerWorld,
    anchor: petramond_math::world_pos::WorldPos,
    checked: &mut FxHashSet<ChunkPos>,
) -> Vec<HerdSpawn> {
    if !mob_census_ready(world, anchor) {
        return Vec::new();
    }
    let center = ChunkPos::new(anchor.x.floor() as i32 >> 4, anchor.z.floor() as i32 >> 4);
    let mut herds = Vec::new();
    let mut budget = ROLL_BUDGET_PER_TICK;
    for dz in -POPULATE_CHUNK_RADIUS..=POPULATE_CHUNK_RADIUS {
        for dx in -POPULATE_CHUNK_RADIUS..=POPULATE_CHUNK_RADIUS {
            let chunk = ChunkPos::new(center.cx + dx, center.cz + dz);
            if checked.contains(&chunk) {
                continue;
            }
            if !world.data().chunk_loaded(chunk.cx, chunk.cz) {
                continue;
            }
            if world.column_populated(chunk) {
                checked.insert(chunk);
                continue;
            }
            let mut rng = chunk_rng(world.data().seed, chunk);
            let draw = rng.next_f32();
            if draw >= POPULATE_CHANCE || !wins_spacing(world.data().seed, chunk, draw.to_bits()) {
                checked.insert(chunk);
                continue;
            }
            if budget == 0 {
                return herds;
            }
            budget -= 1;
            checked.insert(chunk);
            if let Some(spawns) = place_herd(world, chunk, &mut rng) {
                herds.push(HerdSpawn { chunk, spawns });
            }
        }
    }
    herds
}

fn place_herd(world: &ServerWorld, chunk: ChunkPos, rng: &mut MobRng) -> Option<Vec<Spawn>> {
    let (kind, first) = anchor_member(world, chunk, rng)?;
    let (ax, az) = (first.pos.x.floor() as i32, first.pos.z.floor() as i32);
    if !biome_chance_passes(world, kind, ax, az, rng) {
        return None;
    }
    let want = def(kind).spawn_group.roll(rng);
    let origin = IVec3::new(first.pos.x.floor() as i32, 0, first.pos.z.floor() as i32);
    let mut spawns = vec![first];
    while (spawns.len() as u32) < want {
        let Some(next) = nearby_spawn(world, kind, origin, &spawns, rng, &site_for) else {
            break;
        };
        spawns.push(next);
    }
    Some(spawns)
}

fn anchor_member(world: &ServerWorld, chunk: ChunkPos, rng: &mut MobRng) -> Option<(Mob, Spawn)> {
    for _ in 0..SITE_TRIES {
        let wx = chunk.cx * CHUNK_SX as i32 + rng.next_range(0, CHUNK_SX as i32 - 1);
        let wz = chunk.cz * CHUNK_SZ as i32 + rng.next_range(0, CHUNK_SZ as i32 - 1);
        let Some(kind) = choose_kind_for_site(world, wx, wz, rng) else {
            continue;
        };
        if let Some(spawn) = spawn_with(world, kind, wx, wz, rng, &site_for) {
            return Some((kind, spawn));
        }
    }
    None
}

fn choose_kind_for_site(world: &ServerWorld, wx: i32, wz: i32, rng: &mut MobRng) -> Option<Mob> {
    let disabled = world.data().disabled_mods();
    let mut chosen = None;
    let mut seen = 0i32;
    for d in defs() {
        if d.category != MobCategory::Passive
            || !d.spawn.is_spawnable()
            || !species_enabled(d.mob, disabled)
        {
            continue;
        }
        if site_for(world, d.mob, wx, wz).is_none() {
            continue;
        }
        seen += 1;
        if rng.next_range(0, seen - 1) == 0 {
            chosen = Some(d.mob);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::biome::Biome;
    use petramond_world::block::Block;
    use petramond_world::chunk::Chunk;

    fn grass_world(seed: u32) -> ServerWorld {
        let mut world = ServerWorld::new(seed, 1);
        for (cx, cz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            let mut chunk = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_block(x, 64, z, Block::Grass);
                    chunk.set_biome(x, z, Biome::PLAINS.id());
                }
            }
            world.insert_chunk_for_test(ChunkPos::new(cx, cz), chunk);
        }
        world
    }

    fn anchor() -> WorldPos {
        WorldPos::new(8.0, 65.0, 8.0)
    }

    fn populating_seed() -> u32 {
        let anchor_chunk = ChunkPos::new(0, 0);
        (0..10_000u32)
            .find(|&s| {
                let mut checked = FxHashSet::default();
                attempt(&grass_world(s), anchor(), &mut checked)
                    .iter()
                    .any(|h| h.chunk == anchor_chunk)
            })
            .expect("some small seed places a herd on chunk (0,0)")
    }

    #[test]
    fn herd_roll_is_deterministic_per_seed_and_terrain() {
        let seed = populating_seed();
        let collect = |world: &ServerWorld| {
            let mut checked = FxHashSet::default();
            attempt(world, anchor(), &mut checked)
                .into_iter()
                .flat_map(|h| {
                    let chunk = h.chunk;
                    h.spawns
                        .into_iter()
                        .map(move |s| (chunk, s.kind, s.pos.x, s.pos.y, s.pos.z, s.yaw))
                })
                .collect::<Vec<_>>()
        };

        let a = collect(&grass_world(seed));
        let b = collect(&grass_world(seed));

        assert!(
            !a.is_empty(),
            "the searched seed populates the anchor chunk"
        );
        assert_eq!(a, b, "same seed + same terrain places identical herds");
    }

    #[test]
    fn herd_chunks_keep_their_spacing() {
        let seed = 12345;
        let winners: Vec<ChunkPos> = (-20..20)
            .flat_map(|cz| (-20..20).map(move |cx| ChunkPos::new(cx, cz)))
            .filter(|&c| herd_draw(seed, c).is_some_and(|own| wins_spacing(seed, c, own)))
            .collect();

        assert!(
            winners.len() > 10,
            "a 40x40 region keeps a healthy herd count, got {}",
            winners.len()
        );
        for (i, a) in winners.iter().enumerate() {
            for b in &winners[i + 1..] {
                let dist = (a.cx - b.cx).abs().max((a.cz - b.cz).abs());
                assert!(
                    dist > HERD_SPACING_CHUNKS,
                    "herd chunks {a:?} and {b:?} violate the spacing radius"
                );
            }
        }
    }

    #[test]
    fn a_checked_chunk_is_not_rerolled_within_the_session() {
        let seed = populating_seed();
        let world = grass_world(seed);
        let mut checked = FxHashSet::default();

        assert!(!attempt(&world, anchor(), &mut checked).is_empty());
        assert!(
            attempt(&world, anchor(), &mut checked).is_empty(),
            "the session memo stops the scan from re-rolling settled chunks"
        );
    }

    #[test]
    fn a_populated_chunk_never_repopulates_across_sessions() {
        let seed = populating_seed();
        let mut world = grass_world(seed);
        let spawned = world.populate_mobs_tick(anchor());
        assert!(!spawned.is_empty(), "session one places the worldgen herd");
        let populated = world.populated_columns().clone();
        assert!(populated.contains(&ChunkPos::new(0, 0)));

        let mut world = grass_world(seed);
        world.set_populated_columns(populated);
        let spawned = world.populate_mobs_tick(anchor());
        assert!(spawned.is_empty(), "the one-time stock does not re-mint");
    }

    #[test]
    fn population_ignores_the_trickle_population_caps() {
        let seed = populating_seed();
        let mut world = grass_world(seed);
        for i in 0..MobCategory::Passive.cap() {
            let pos = WorldPos::new(f64::from(8.0 + i as f32 * 0.2), 65.0, 8.0);
            assert!(world.spawn_mob(Mob::Sheep, pos, 0.0).is_some());
        }

        let spawned = world.populate_mobs_tick(anchor());
        assert!(!spawned.is_empty(), "worldgen herds bypass the caps");
    }
}
