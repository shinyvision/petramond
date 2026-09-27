//! Player spawn selection.
//!
//! Drop the player on a random solid surface block near the world origin. Pick a uniformly
//! random dry-land column within [`SEARCH_RADIUS`] of a centre and stand the player on top.
//!
//! Random, not deterministic. The rest of worldgen is a pure function of the seed, but spawn
//! draws from OS entropy each call, so it differs per launch and per player (future
//! multiplayer). Terrain stays seed-deterministic; only where the player lands is random.
//! `find_spawn_rng` takes an explicit `rng_seed` for reproducible tests; [`find_spawn`] feeds it
//! real entropy.
//!
//! Standing-ground predicate: `surf` is a column's top solid height. The chunk filler floods
//! density-air at or below `SEA_LEVEL` to water, so `surf >= SEA_LEVEL` means no sea water on
//! top. That excludes oceans and lakes but accepts beaches, plains and mountains. The surface
//! voxel must also survive the cave carve, otherwise it's a cave mouth with no floor.
//!
//! Choosing the centre: find the nearest dry-land column to the origin first (outward
//! chunk-ring walk). If it's within [`SEARCH_RADIUS`], randomise in a disk around the origin. If
//! it's farther (origin is mid-ocean), move the radius to that nearest coast instead. That
//! nearest-land column is also the fallback if rejection sampling comes up empty, e.g. a tiny
//! island where most random points land in water.

use petramond_math::detmath;
use petramond_world::chunk::SEA_LEVEL;
use petramond_world::mathh::IVec3;

use super::density::surface::SurfaceDensitySystem;
use super::feature::cached_feature_region;
use super::noise::cave_field::CaveField;

pub const SEARCH_RADIUS: i32 = 500;

const MAX_COAST_RADIUS: i32 = 4096;

const MAX_ATTEMPTS: u32 = 256;

const CHUNK: i32 = 16;

pub fn find_spawn(seed: u32) -> IVec3 {
    let world = SpawnWorld::new(seed);
    find_spawn_rng(&world, os_random_u64())
}

fn find_spawn_rng(world: &SpawnWorld, rng_seed: u64) -> IVec3 {
    let mut rng = Rng::new(rng_seed);

    let nearest = match nearest_dry_land(world) {
        None => return IVec3::new(0, SEA_LEVEL, 0),
        Some(p) => p,
    };

    let radius = SEARCH_RADIUS as i64;
    let near_sq = (nearest.x as i64) * (nearest.x as i64) + (nearest.z as i64) * (nearest.z as i64);
    let centre = if near_sq <= radius * radius {
        (0, 0)
    } else {
        (nearest.x, nearest.z)
    };

    sample_dry_land(world, centre, &mut rng).unwrap_or(nearest)
}

struct SpawnWorld {
    surface: SurfaceDensitySystem,
    caves: CaveField,
}

impl SpawnWorld {
    fn new(seed: u32) -> Self {
        Self {
            surface: SurfaceDensitySystem::new(seed),
            caves: CaveField::new(seed),
        }
    }

    fn standing_heights(&self, cx: i32, cz: i32) -> Vec<Option<i32>> {
        let (region, raw) =
            cached_feature_region(&self.surface, &self.caves, cx * CHUNK, cz * CHUNK, 16, 16);
        raw.iter()
            .zip(&region.surf)
            .map(|(&raw, &adjusted)| (raw >= SEA_LEVEL && adjusted == raw).then_some(raw))
            .collect()
    }
}

fn sample_dry_land(world: &SpawnWorld, (cx, cz): (i32, i32), rng: &mut Rng) -> Option<IVec3> {
    let r = SEARCH_RADIUS as f32;
    let r_sq = (SEARCH_RADIUS as i64) * (SEARCH_RADIUS as i64);
    for _ in 0..MAX_ATTEMPTS {
        let radius = r * rng.next_f32().sqrt();
        let theta = std::f32::consts::TAU * rng.next_f32();
        let wx = cx + (radius * detmath::cosf(theta)).round() as i32;
        let wz = cz + (radius * detmath::sinf(theta)).round() as i32;
        let (dx, dz) = ((wx - cx) as i64, (wz - cz) as i64);
        if dx * dx + dz * dz > r_sq {
            continue;
        }
        if let Some(surf) = standing_height(world, wx, wz) {
            return Some(IVec3::new(wx, surf, wz));
        }
    }
    None
}

fn nearest_dry_land(world: &SpawnWorld) -> Option<IVec3> {
    let max_ring = MAX_COAST_RADIUS / CHUNK + 2;
    let mut best: Option<(i64, IVec3)> = None;
    let mut r = 0;
    loop {
        for (cx, cz) in ring_chunks(r) {
            scan_chunk(world, cx, cz, &mut best);
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

fn scan_chunk(world: &SpawnWorld, cx: i32, cz: i32, best: &mut Option<(i64, IVec3)>) {
    let heights = world.standing_heights(cx, cz);
    for z in 0..16i32 {
        for x in 0..16i32 {
            let Some(s) = heights[(z * 16 + x) as usize] else {
                continue;
            };
            let wx = cx * CHUNK + x;
            let wz = cz * CHUNK + z;
            let d = (wx as i64) * (wx as i64) + (wz as i64) * (wz as i64);
            if best.is_none_or(|(bd, _)| d < bd) {
                *best = Some((d, IVec3::new(wx, s, wz)));
            }
        }
    }
}

fn standing_height(world: &SpawnWorld, wx: i32, wz: i32) -> Option<i32> {
    let tcx = wx.div_euclid(CHUNK);
    let tcz = wz.div_euclid(CHUNK);
    let heights = world.standing_heights(tcx, tcz);
    let lx = (wx - tcx * CHUNK) as usize;
    let lz = (wz - tcz * CHUNK) as usize;
    heights[lz * 16 + lx]
}

fn ring_chunks(r: i32) -> Vec<(i32, i32)> {
    if r == 0 {
        return vec![(0, 0)];
    }
    let mut v = Vec::with_capacity((8 * r) as usize);
    for cx in -r..=r {
        v.push((cx, -r));
        v.push((cx, r));
    }
    for cz in (-r + 1)..=(r - 1) {
        v.push((-r, cz));
        v.push((r, cz));
    }
    v
}

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

    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
}

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

    #[test]
    fn find_spawn_rng_is_deterministic() {
        for &seed in &SEEDS {
            let world = SpawnWorld::new(seed);
            for rng_seed in [0u64, 1, 99, 1_000_000] {
                assert_eq!(
                    find_spawn_rng(&world, rng_seed),
                    find_spawn_rng(&world, rng_seed),
                    "seed {seed:#x} rng {rng_seed}"
                );
            }
        }
    }

    #[test]
    fn spawns_stand_on_uncarved_dry_land() {
        for &seed in &SEEDS {
            let world = SpawnWorld::new(seed);
            for rng_seed in [0u64, 1, 99, 1_000_000] {
                let p = find_spawn_rng(&world, rng_seed);
                assert!(
                    p.y >= SEA_LEVEL,
                    "seed {seed:#x} rng {rng_seed}: spawned over water"
                );
                assert_eq!(
                    world.caves.feature_surface_after_caves(p.x, p.z, p.y),
                    p.y,
                    "seed {seed:#x} rng {rng_seed}: spawned over a cave mouth"
                );
            }
        }
    }
}
