//! Player spawn selection.
//!
//! Goal: drop the player onto a *random* solid block on the surface near the
//! world origin. Concretely we pick a uniformly random column within
//! [`SEARCH_RADIUS`] of a centre whose top solid block is exposed to the sky
//! (dry land), then stand the player on top of it.
//!
//! **Random, not deterministic.** Unlike the rest of worldgen (which is a pure
//! function of the seed), the spawn point is drawn from OS entropy on every call
//! — a fresh location each launch, and a different location for each player when
//! several share one world (future multiplayer). The terrain stays fully
//! seed-deterministic; only *where on it* the player lands is random. The random
//! core [`find_spawn_rng`] takes an explicit `rng_seed` so tests stay
//! reproducible; [`find_spawn`] feeds it real entropy.
//!
//! **The dry-land predicate.** A column's `surf` is its top *solid* surface
//! height; the chunk filler floods every empty cell with `y > surf && y <=
//! SEA_LEVEL` to water. River carving only ever *lowers* `surf` toward the
//! channel bed (water still fills up to `SEA_LEVEL`), and ocean/lake floors are
//! generated below sea level to begin with. So after river carving, the single
//! test `surf >= SEA_LEVEL` means "the top solid block has no water above it" —
//! it cleanly excludes oceans, lakes, and river channels while accepting beaches
//! (`surf == SEA_LEVEL`), plains, and mountains. That is exactly "a solid block
//! on the surface".
//!
//! **Choosing the centre.** We first find the nearest dry-land column to the
//! origin (an outward chunk-ring walk). If it lies within [`SEARCH_RADIUS`] the
//! origin has land in range, so we randomise within a disk centred on the
//! origin. If the nearest land is farther — the origin is in the middle of a
//! large ocean — we move the radius onto that nearest coast and randomise there.
//! That nearest-land column is also the guaranteed fallback if rejection
//! sampling somehow comes up empty (e.g. a tiny island where almost every random
//! point lands in water).

use crate::chunk::SEA_LEVEL;
use crate::mathh::IVec3;

use super::classic::world::CascadeWorld;
use super::river::RiverSystem;

/// Radius (blocks) of the disk a spawn is drawn from, around the origin (or the
/// nearest coast when the origin is open ocean).
pub const SEARCH_RADIUS: i32 = 500;

/// Hard backstop (blocks) for the nearest-coast walk when the origin is in open
/// ocean. Far beyond any ocean this generator produces; only guards against an
/// unbounded scan on a degenerate all-water seed.
const MAX_COAST_RADIUS: i32 = 4096;

/// How many random points to try before giving up on a centre and falling back
/// to the nearest-land column. Only ever exhausted when a centre is nearly
/// surrounded by water (e.g. a tiny island), so the cost is bounded and rare.
const MAX_ATTEMPTS: u32 = 256;

const CHUNK: i32 = 16;

/// Pick a random dry-land surface block to spawn on, using fresh OS entropy.
/// Returns the **solid surface block** `(x, surf_y, z)`; stand the player's feet
/// at `surf_y + 1`. See the module docs for centre selection and the ocean
/// fallback.
///
/// `world` is borrowed so the caller can share its existing [`CascadeWorld`];
/// `seed` is needed to build the matching river overlay.
pub fn find_spawn(world: &CascadeWorld, seed: u32) -> IVec3 {
    find_spawn_rng(world, seed, os_random_u64())
}

/// [`find_spawn`] with an explicit random seed instead of OS entropy: the result
/// is a pure function of `(seed, rng_seed)`, so tests are reproducible.
fn find_spawn_rng(world: &CascadeWorld, seed: u32, rng_seed: u64) -> IVec3 {
    let rivers = RiverSystem::new(seed);
    let mut rng = Rng::new(rng_seed);

    let nearest = match nearest_dry_land(world, &rivers) {
        // Degenerate: no land within MAX_COAST_RADIUS. Stand at the origin water
        // surface rather than hang or panic.
        None => return IVec3::new(0, SEA_LEVEL, 0),
        Some(p) => p,
    };

    // Land within range of the origin → randomise around the origin. Otherwise
    // the origin is open ocean → move the radius onto the nearest coast.
    let radius = SEARCH_RADIUS as i64;
    let near_sq = (nearest.x as i64) * (nearest.x as i64) + (nearest.z as i64) * (nearest.z as i64);
    let centre = if near_sq <= radius * radius {
        (0, 0)
    } else {
        (nearest.x, nearest.z)
    };

    sample_dry_land(world, &rivers, centre, &mut rng).unwrap_or(nearest)
}

/// Try up to [`MAX_ATTEMPTS`] uniformly random columns inside the disk of radius
/// [`SEARCH_RADIUS`] around `centre`, returning the first that is dry land.
fn sample_dry_land(
    world: &CascadeWorld,
    rivers: &RiverSystem,
    (cx, cz): (i32, i32),
    rng: &mut Rng,
) -> Option<IVec3> {
    let r = SEARCH_RADIUS as f32;
    let r_sq = (SEARCH_RADIUS as i64) * (SEARCH_RADIUS as i64);
    for _ in 0..MAX_ATTEMPTS {
        // Uniform over the disk's area: radius ∝ sqrt(u) spreads points evenly
        // instead of clustering them near the centre.
        let radius = r * rng.next_f32().sqrt();
        let theta = std::f32::consts::TAU * rng.next_f32();
        let wx = cx + (radius * theta.cos()).round() as i32;
        let wz = cz + (radius * theta.sin()).round() as i32;
        let (dx, dz) = ((wx - cx) as i64, (wz - cz) as i64);
        if dx * dx + dz * dz > r_sq {
            continue; // rounding nudged it just outside the radius — retry.
        }
        let surf = column_surface(world, rivers, wx, wz);
        if surf >= SEA_LEVEL {
            return Some(IVec3::new(wx, surf, wz));
        }
    }
    None
}

/// Nearest dry-land column to the origin, by true (Euclidean) distance, via an
/// outward chunk-ring walk. Used both to decide the spawn centre and as the
/// guaranteed fallback. `None` only on a degenerate all-ocean seed.
///
/// Because the origin is the min corner of chunk `(0, 0)`, the nearest column in
/// chunk ring `r` can be as close as `16r - 15` blocks (the inner edge of the
/// negative-side chunk), so the closest any *unscanned* ring (`>= r+1`) can hold
/// is `16r + 1`. Once the best is at least that close we stop — exact nearest,
/// terminating within ~one extra ring of finding land.
fn nearest_dry_land(world: &CascadeWorld, rivers: &RiverSystem) -> Option<IVec3> {
    let max_ring = MAX_COAST_RADIUS / CHUNK + 2;
    let mut best: Option<(i64, IVec3)> = None;
    let mut r = 0;
    loop {
        for (cx, cz) in ring_chunks(r) {
            scan_chunk(world, rivers, cx, cz, &mut best);
        }
        if let Some((best_sq, _)) = best {
            let next_min = (CHUNK as i64) * (r as i64) + 1;
            if best_sq <= next_min * next_min {
                break;
            }
        }
        r += 1;
        if r > max_ring {
            break;
        }
    }
    best.map(|(_, p)| p)
}

/// Scan one chunk's 16x16 columns, updating `best` with any dry-land column
/// closer to the origin than the current best.
///
/// River carving only ever lowers `surf`, so a column can only end up dry land
/// if its *base* surface is already at/above sea level. We therefore generate
/// the cheap base region first and only pay for `rivers.apply` on chunks that
/// actually contain land — pure-ocean chunks (the bulk of a large ocean) skip it
/// — while the carved surface still decides which coastal columns are dry.
fn scan_chunk(
    world: &CascadeWorld,
    rivers: &RiverSystem,
    cx: i32,
    cz: i32,
    best: &mut Option<(i64, IVec3)>,
) {
    let mut region = world.region(cx * CHUNK, cz * CHUNK, 16, 16);
    if !region.surf.iter().any(|&s| s >= SEA_LEVEL) {
        return; // all ocean floor — no possible dry-land column here.
    }
    rivers.apply(&mut region);
    for z in 0..16i32 {
        for x in 0..16i32 {
            let surf = region.surf[(z * 16 + x) as usize];
            if surf < SEA_LEVEL {
                continue; // ocean / lake / river channel — surface is water.
            }
            let wx = cx * CHUNK + x;
            let wz = cz * CHUNK + z;
            let d = (wx as i64) * (wx as i64) + (wz as i64) * (wz as i64);
            if best.is_none_or(|(bd, _)| d < bd) {
                *best = Some((d, IVec3::new(wx, surf, wz)));
            }
        }
    }
}

/// River-carved top-solid surface height for a single world column. Matches the
/// per-chunk batch [`scan_chunk`] reads: river carving is a pure per-column
/// function of the seed, so the covering region's size and origin do not change
/// the result. Skips river carving on ocean-floor columns (rivers only lower
/// `surf`, so a sub-sea-level base can never become dry land).
fn column_surface(world: &CascadeWorld, rivers: &RiverSystem, wx: i32, wz: i32) -> i32 {
    let x0 = wx.div_euclid(4) * 4;
    let z0 = wz.div_euclid(4) * 4;
    let mut region = world.region(x0, z0, 4, 4);
    if region.at(wx, wz).0 < SEA_LEVEL {
        return region.at(wx, wz).0;
    }
    rivers.apply(&mut region);
    region.at(wx, wz).0
}

/// Chunk coordinates on the square ring at Chebyshev distance `r` from `(0, 0)`.
fn ring_chunks(r: i32) -> Vec<(i32, i32)> {
    if r == 0 {
        return vec![(0, 0)];
    }
    let mut v = Vec::with_capacity((8 * r) as usize);
    for cx in -r..=r {
        v.push((cx, -r)); // top edge
        v.push((cx, r)); // bottom edge
    }
    for cz in (-r + 1)..=(r - 1) {
        v.push((-r, cz)); // left edge (corners already covered above)
        v.push((r, cz)); // right edge
    }
    v
}

/// Stateful SplitMix64 — the codebase's deterministic hash finalizer (see
/// `entity::hash01`) advanced as a stream. Used only to draw the random sample
/// points; the stream is seeded from OS entropy in production.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform `f32` in `[0, 1)` from the top 24 mantissa bits.
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
}

/// A fresh 64-bit value from OS entropy, varying per process (and per call).
/// Drawn from `RandomState`'s OS-seeded hash keys.
fn os_random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEEDS: [u32; 5] = [0x1234_5678, 1, 7, 0xDEAD_BEEF, 42];

    fn dist_sq(p: IVec3) -> i64 {
        (p.x as i64) * (p.x as i64) + (p.z as i64) * (p.z as i64)
    }

    #[test]
    fn spawn_is_dry_land() {
        for &seed in &SEEDS {
            let world = CascadeWorld::new(seed);
            let rivers = RiverSystem::new(seed);
            for rng_seed in 0..8u64 {
                let p = find_spawn_rng(&world, seed, rng_seed);
                let surf = column_surface(&world, &rivers, p.x, p.z);
                assert!(
                    surf >= SEA_LEVEL,
                    "seed {seed:#x} rng {rng_seed}: spawn ({}, {}) surface {surf} below sea level",
                    p.x,
                    p.z
                );
                assert_eq!(
                    surf, p.y,
                    "seed {seed:#x} rng {rng_seed}: y must be the carved surface"
                );
            }
        }
    }

    #[test]
    fn find_spawn_rng_is_deterministic() {
        for &seed in &SEEDS {
            let world = CascadeWorld::new(seed);
            for rng_seed in [0u64, 1, 99, 1_000_000] {
                assert_eq!(
                    find_spawn_rng(&world, seed, rng_seed),
                    find_spawn_rng(&world, seed, rng_seed),
                    "seed {seed:#x} rng {rng_seed}"
                );
            }
        }
    }

    #[test]
    fn spawn_is_within_radius_of_origin_when_land_is_near() {
        // When the nearest land is within SEARCH_RADIUS of the origin, the disk is
        // centred on the origin, so every spawn must be within that radius.
        let r_sq = (SEARCH_RADIUS as i64) * (SEARCH_RADIUS as i64);
        for &seed in &SEEDS {
            let world = CascadeWorld::new(seed);
            let rivers = RiverSystem::new(seed);
            let Some(nearest) = nearest_dry_land(&world, &rivers) else {
                continue;
            };
            if dist_sq(nearest) > r_sq {
                continue; // origin is open ocean — centre moves to the coast.
            }
            for rng_seed in 0..16u64 {
                let p = find_spawn_rng(&world, seed, rng_seed);
                assert!(
                    dist_sq(p) <= r_sq,
                    "seed {seed:#x} rng {rng_seed}: spawn ({}, {}) outside {SEARCH_RADIUS} of origin",
                    p.x,
                    p.z
                );
            }
        }
    }

    #[test]
    fn different_rng_seeds_spread_the_spawn() {
        // A world with land around the origin should scatter spawns across many
        // distinct columns rather than always returning the same point.
        let seed = 3u32; // origin is land (see diag_spawn_report).
        let world = CascadeWorld::new(seed);
        let mut seen = std::collections::HashSet::new();
        let mut max_d = 0i64;
        for rng_seed in 0..200u64 {
            let p = find_spawn_rng(&world, seed, rng_seed);
            seen.insert((p.x, p.z));
            max_d = max_d.max(dist_sq(p));
        }
        assert!(
            seen.len() > 50,
            "expected varied spawns, got {} distinct",
            seen.len()
        );
        assert!(
            max_d > 100 * 100,
            "expected spawns spread across the radius, max dist {}",
            (max_d as f64).sqrt()
        );
    }

    #[test]
    #[ignore = "diagnostic: prints spawn placement, distance and timing per seed"]
    fn diag_spawn_report() {
        for seed in 0u32..16 {
            let world = CascadeWorld::new(seed);
            let rivers = RiverSystem::new(seed);
            let origin = column_surface(&world, &rivers, 0, 0);
            let t0 = std::time::Instant::now();
            let p = find_spawn_rng(&world, seed, 0xC0FFEE);
            let dt = t0.elapsed();
            let dist = (dist_sq(p) as f64).sqrt();
            let surf = column_surface(&world, &rivers, p.x, p.z);
            eprintln!(
                "seed {seed:>2}: origin_surf {origin:>3} ({}) -> spawn ({:>5},{:>3},{:>5}) surf {surf:>3} dist {dist:>7.1}  in {:?}",
                if origin >= SEA_LEVEL { "LAND " } else { "OCEAN" },
                p.x,
                p.y,
                p.z,
                dt,
            );
        }
    }
}
