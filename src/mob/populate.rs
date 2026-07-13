//! One-time worldgen passive herds — the world's initial animal stock.
//!
//! Terrain regenerates from seed every session, so "chunk generation time" is
//! not a one-shot event here the way it is in persistent-chunk games. Instead
//! each chunk column gets a DETERMINISTIC roll — a pure function of
//! `(world seed, chunk)` — deciding whether it hosts a herd, of which species,
//! and where. The roll is evaluated lazily, once per session per chunk, as
//! loaded chunks near a player pass the mob census; a chunk that actually
//! placed a herd is recorded in the world's persisted populated set (rides
//! `level.dat`) so the stock never re-mints on later sessions. A chunk whose
//! roll placed nothing is deliberately NOT recorded: the same seed re-rolls
//! the same nothing next time, for free.
//!
//! Population is worldgen stock, so it deliberately IGNORES the natural-spawn
//! population caps: geometry bounds it instead — the [`POPULATE_CHANCE`] roll
//! thinned by [`HERD_SPACING_CHUNKS`] suppression (~4% of chunks host a herd,
//! never two herds within the spacing radius) within
//! [`POPULATE_CHUNK_RADIUS`] of a player. The runtime spawner
//! ([`super::spawn`]) stays the slow cap-limited backfill trickle. Killing the
//! stock is meant to be a non-renewable harvest — more animals should cost
//! travel (virgin chunks) or, eventually, breeding.

use rustc_hash::FxHashSet;

use crate::chunk::{ChunkPos, CHUNK_SX, CHUNK_SZ};
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::spawn::{
    mob_census_ready, nearby_spawn, site_for, spawn_with, species_enabled, splitmix, Spawn,
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
/// Chebyshev chunk radius around each player anchor that gets populated. Must
/// stay inside the nine-chunk census square the attempt is gated on, so every
/// candidate's saved records (which may carry the herd's survivors) have
/// already applied.
const POPULATE_CHUNK_RADIUS: i32 = 8;
/// At most this many herd rolls (chance already passed) run per tick — bounds
/// the site-probing cost of a world join, where a whole disc of chunks becomes
/// eligible at once. Chance-failed chunks cost one hash probe + one RNG draw
/// and are not metered.
const ROLL_BUDGET_PER_TICK: u32 = 8;
/// Candidate anchor sites tried within the chunk before the roll gives up.
const SITE_TRIES: u32 = 8;
/// Decorrelates the population stream from every other consumer of the seed.
const POPULATE_SALT: u64 = 0x0F0F_5EED_4E7D_0001;

/// A herd the manager should place, tagged with the chunk to record as
/// populated once at least one member actually spawned.
pub(super) struct HerdSpawn {
    pub chunk: ChunkPos,
    pub spawns: Vec<Spawn>,
}

/// The deterministic per-chunk stream: pure in `(seed, chunk)`, sign-extended
/// mixing in the spirit of the worldgen positional-seeding contract.
fn chunk_rng(seed: u32, chunk: ChunkPos) -> MobRng {
    let mixed = splitmix(
        (seed as u64)
            ^ POPULATE_SALT
            ^ (chunk.cx as i64 as u64).wrapping_mul(0x632B_E599_37D5_ACE5)
            ^ (chunk.cz as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
    );
    MobRng::new(mixed)
}

/// A chunk's raw chance draw when it passes [`POPULATE_CHANCE`], as the f32's
/// bit pattern (monotonic for the non-negative draw, so it orders like the
/// float but is `Ord`). `None` = the chunk rolls no herd.
fn herd_draw(seed: u32, chunk: ChunkPos) -> Option<u32> {
    let d = chunk_rng(seed, chunk).next_f32();
    (d < POPULATE_CHANCE).then(|| d.to_bits())
}

/// Whether `chunk`'s passing draw survives spacing suppression: it must be the
/// strongest (lowest, coord-tiebroken) passing draw within
/// [`HERD_SPACING_CHUNKS`]. Pure in `(seed, chunk)` like the draw itself.
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

/// Run one population step around `anchor`: scan its chunk square for chunks
/// not yet checked this session, and roll a budgeted batch of them. `checked`
/// is the per-session memo — a chunk enters it when its roll COMPLETED
/// (chance failed, or placement ran against final terrain), never when it was
/// merely skipped as unloaded, so frontier chunks retry as they stream in.
pub(super) fn attempt(
    world: &World,
    anchor: Vec3,
    checked: &mut FxHashSet<ChunkPos>,
) -> Vec<HerdSpawn> {
    // Same gate as the trickle: until every nearby column landed and every
    // saved record applied, the live world is not final — records may still
    // carry this very herd's survivors from an earlier session.
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
            // Unloaded (e.g. a square corner outside the streamable disc):
            // skip WITHOUT checking off, so it rolls when it streams in.
            if !world.chunk_loaded(chunk.cx, chunk.cz) {
                continue;
            }
            if world.column_populated(chunk) {
                checked.insert(chunk);
                continue;
            }
            let mut rng = chunk_rng(world.seed, chunk);
            let draw = rng.next_f32();
            if draw >= POPULATE_CHANCE || !wins_spacing(world.seed, chunk, draw.to_bits()) {
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

/// Roll the herd itself: an anchor site inside the chunk, the species the
/// site's biome/ground admits, the group size, then members placed like a
/// natural group — but with no player-distance band (the herd is "already
/// there" when the player arrives) and no population caps.
fn place_herd(world: &World, chunk: ChunkPos, rng: &mut MobRng) -> Option<Vec<Spawn>> {
    let (kind, first) = anchor_member(world, chunk, rng)?;
    let want = def(kind).spawn_group.roll(rng);
    let origin = IVec3::new(first.pos.x.floor() as i32, 0, first.pos.z.floor() as i32);
    let mut spawns = vec![first];
    while (spawns.len() as u32) < want {
        let Some(next) = nearby_spawn(world, kind, origin, &spawns, rng, &site_for) else {
            break; // keep the partial herd; the chunk still counts as populated
        };
        spawns.push(next);
    }
    Some(spawns)
}

/// Find the herd's first member: a valid foothold in the chunk plus a species
/// whose spawn rule admits that site. Site-first (the biome decides what lives
/// there), species drawn uniformly among the admitting passive rows.
fn anchor_member(world: &World, chunk: ChunkPos, rng: &mut MobRng) -> Option<(Mob, Spawn)> {
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

/// Pick uniformly among the passive, naturally-spawnable, enabled species whose
/// rule admits this exact site (reservoir sampling, like the trickle's picker —
/// but per site instead of per population room).
fn choose_kind_for_site(world: &World, wx: i32, wz: i32, rng: &mut MobRng) -> Option<Mob> {
    let disabled = world.disabled_mods();
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
    use crate::biome::Biome;
    use crate::block::Block;
    use crate::chunk::Chunk;

    /// A census-ready flat grass neighborhood: the anchor's chunk plus the four
    /// columns of the render-distance-1 streamable disc.
    fn grass_world(seed: u32) -> World {
        let mut world = World::new(seed, 1);
        for (cx, cz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            let mut chunk = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_block(x, 64, z, Block::Grass);
                    chunk.set_biome(x, z, Biome::Plains.id());
                }
            }
            world.insert_chunk_for_test(ChunkPos::new(cx, cz), chunk);
        }
        world
    }

    fn anchor() -> Vec3 {
        Vec3::new(8.0, 65.0, 8.0)
    }

    /// A seed whose deterministic roll populates the anchor chunk (chance AND
    /// spacing) — searched, not pinned, so retuning `POPULATE_CHANCE` or the
    /// spacing radius can't break these tests.
    fn populating_seed() -> u32 {
        let anchor_chunk = ChunkPos::new(0, 0);
        (0..10_000u32)
            .find(|&s| {
                herd_draw(s, anchor_chunk).is_some_and(|own| wins_spacing(s, anchor_chunk, own))
            })
            .expect("some small seed rolls a herd for chunk (0,0)")
    }

    #[test]
    fn herd_roll_is_deterministic_per_seed_and_terrain() {
        let seed = populating_seed();
        let collect = |world: &World| {
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
        // Pure positional sweep — no world needed. Across a big region, no two
        // surviving herd chunks may sit within the spacing radius of each
        // other, and the suppression must still let a healthy share through.
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

        // "Next session": terrain regenerates identically and the live mobs are
        // gone; only the persisted populated set survives. The stock must not
        // re-mint — this is the whole anti-farm invariant.
        let mut world = grass_world(seed);
        world.set_populated_columns(populated);
        let spawned = world.populate_mobs_tick(anchor());
        assert!(spawned.is_empty(), "the one-time stock does not re-mint");
    }

    #[test]
    fn population_ignores_the_trickle_population_caps() {
        let seed = populating_seed();
        let mut world = grass_world(seed);
        // Saturate the passive category cap. Worldgen stock is bounded by
        // geometry (chance × radius), not by the trickle's caps — otherwise
        // travelling at cap would silently leave new terrain barren.
        for i in 0..MobCategory::Passive.cap() {
            let pos = Vec3::new(8.0 + i as f32 * 0.2, 65.0, 8.0);
            assert!(world.spawn_mob(Mob::Sheep, pos, 0.0));
        }

        let spawned = world.populate_mobs_tick(anchor());
        assert!(!spawned.is_empty(), "worldgen herds bypass the caps");
    }
}
