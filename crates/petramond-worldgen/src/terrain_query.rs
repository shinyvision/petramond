//! Material queries share the generation cache, including cave surface courses.

use petramond_world::chunk::{section_idx, SectionPos};
use petramond_world::section::BlockCube;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

type HeightsKey = (u32, [i32; 2]);
static HEIGHTS: LazyLock<crate::memo::SharedMemo<HeightsKey, Arc<[i32]>>> =
    LazyLock::new(|| crate::memo::SharedMemo::new(8192));

pub(crate) fn height_tile(seed: u32, cell: [i32; 2]) -> Arc<[i32]> {
    HEIGHTS.get_or_insert((seed, cell), || raw_region(seed, cell).surf.clone().into())
}

/// Highest solid density cell before caves and feature placement.
pub fn heights_at(seed: u32, columns: &[[i32; 2]]) -> Vec<i32> {
    let mut tiles = BTreeMap::new();
    columns
        .iter()
        .map(|&[x, z]| {
            let cell = [x.div_euclid(16), z.div_euclid(16)];
            let heights = tiles.entry(cell).or_insert_with(|| height_tile(seed, cell));
            heights[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize]
        })
        .collect()
}

/// The section whose every cell `positions` lists in section order, if any:
/// the shape a tile-caching reader asks in, answered as one cube read.
fn whole_section(positions: &[[i32; 3]]) -> Option<[i32; 3]> {
    use petramond_world::chunk::{SECTION_SIZE, WORLD_MAX_Y, WORLD_MIN_Y};
    const N: usize = SECTION_SIZE;
    let origin = *positions.first()?;
    if positions.len() != N * N * N
        || origin.iter().any(|v| v.rem_euclid(N as i32) != 0)
        || origin[1] < WORLD_MIN_Y
        || origin[1] + N as i32 > WORLD_MAX_Y
        || super::clamp_query(origin) != origin
        || super::clamp_query(origin.map(|v| v + N as i32 - 1)) != origin.map(|v| v + N as i32 - 1)
    {
        return None;
    }
    positions
        .iter()
        .enumerate()
        .all(|(i, p)| {
            *p == [
                origin[0] + (i % N) as i32,
                origin[1] + (i / (N * N)) as i32,
                origin[2] + (i / N % N) as i32,
            ]
        })
        .then(|| origin.map(|v| v.div_euclid(N as i32)))
}

/// [`blocks_at`] over one whole section in section order. A section outside
/// the world answers as the clamped positions would.
pub fn section_blocks(seed: u32, section: [i32; 3]) -> Vec<u16> {
    use petramond_world::chunk::{SECTION_SIZE, WORLD_MAX_Y, WORLD_MIN_Y};
    const N: i32 = SECTION_SIZE as i32;
    let origin = section.map(|v| v.saturating_mul(N));
    let in_world = section[1] * N >= WORLD_MIN_Y
        && section[1] * N + N <= WORLD_MAX_Y
        && super::clamp_query(origin) == origin
        && super::clamp_query(origin.map(|v| v + N - 1)) == origin.map(|v| v + N - 1);
    if !in_world {
        let positions: Vec<[i32; 3]> = (0..N * N * N)
            .map(|i| {
                [
                    origin[0] + i % N,
                    origin[1] + i / (N * N),
                    origin[2] + i / N % N,
                ]
            })
            .collect();
        return blocks_at(seed, &positions);
    }
    let surface = super::surface_system(seed);
    let caves = super::cave_field(seed);
    let region = raw_region(seed, [section[0], section[2]]);
    let biomes: Vec<u8> = region.biomes.iter().map(|b| b.id()).collect();
    super::section_memo::terrain_cube(
        &surface,
        &caves,
        seed,
        SectionPos::new(section[0], section[1], section[2]),
        &biomes,
        &region.surf,
    )
    .iter()
    .collect()
}

pub fn blocks_at(seed: u32, positions: &[[i32; 3]]) -> Vec<u16> {
    let surface = super::surface_system(seed);
    let caves = super::cave_field(seed);
    if let Some(cell) = whole_section(positions) {
        let region = raw_region(seed, [cell[0], cell[2]]);
        let biomes: Vec<u8> = region.biomes.iter().map(|b| b.id()).collect();
        return super::section_memo::terrain_cube(
            &surface,
            &caves,
            seed,
            SectionPos::new(cell[0], cell[1], cell[2]),
            &biomes,
            &region.surf,
        )
        .iter()
        .collect();
    }
    let mut columns = BTreeMap::new();
    let mut cubes: BTreeMap<[i32; 3], BlockCube> = BTreeMap::new();
    positions
        .iter()
        .map(|&pos| {
            let [x, y, z] = super::clamp_query(pos);
            let cell = [x, y, z].map(|v| v.div_euclid(16));
            let cube = cubes.entry(cell).or_insert_with(|| {
                let (biomes, raw) = columns.entry([cell[0], cell[2]]).or_insert_with(|| {
                    let region = raw_region(seed, [cell[0], cell[2]]);
                    (
                        region.biomes.iter().map(|b| b.id()).collect::<Vec<_>>(),
                        region.surf.clone(),
                    )
                });
                super::section_memo::terrain_cube(
                    &surface,
                    &caves,
                    seed,
                    SectionPos::new(cell[0], cell[1], cell[2]),
                    biomes,
                    raw,
                )
            });
            cube.get(section_idx(
                x.rem_euclid(16) as usize,
                y.rem_euclid(16) as usize,
                z.rem_euclid(16) as usize,
            ))
        })
        .collect()
}

type RegionKey = (u32, [i32; 2]);
static REGIONS: LazyLock<crate::memo::SharedMemo<RegionKey, Arc<crate::region::RegionCells>>> =
    LazyLock::new(|| crate::memo::SharedMemo::new(8192));

fn raw_region(seed: u32, cell: [i32; 2]) -> Arc<crate::region::RegionCells> {
    REGIONS.get_or_insert((seed, cell), || {
        Arc::new(super::surface_system(seed).region(cell[0] * 16, cell[1] * 16, 16, 16))
    })
}
