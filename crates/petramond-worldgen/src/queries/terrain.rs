use petramond_world::chunk::{section_idx, SectionPos};
use petramond_world::section::BlockCube;
use std::collections::BTreeMap;

pub fn heights_at(seed: u32, columns: &[[i32; 2]]) -> Vec<i32> {
    let generator = crate::driver::ChunkGenerator::shared(seed);
    let (_, caves) = generator.sources();
    let mut tiles = BTreeMap::new();
    columns
        .iter()
        .map(|&[x, z]| {
            let cell = [x.div_euclid(16), z.div_euclid(16)];
            let heights = tiles
                .entry(cell)
                .or_insert_with(|| caves.density_surface_tile(cell));
            heights[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize]
        })
        .collect()
}

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
    let generator = crate::driver::ChunkGenerator::shared(seed);
    let (surface, caves) = generator.sources();
    crate::section_memo::terrain_cube(
        surface,
        caves,
        SectionPos::new(section[0], section[1], section[2]),
    )
    .iter()
    .collect()
}

pub fn blocks_at(seed: u32, positions: &[[i32; 3]]) -> Vec<u16> {
    let generator = crate::driver::ChunkGenerator::shared(seed);
    let (surface, caves) = generator.sources();
    if let Some(cell) = whole_section(positions) {
        return crate::section_memo::terrain_cube(
            surface,
            caves,
            SectionPos::new(cell[0], cell[1], cell[2]),
        )
        .iter()
        .collect();
    }
    let mut cubes: BTreeMap<[i32; 3], BlockCube> = BTreeMap::new();
    positions
        .iter()
        .map(|&pos| {
            let [x, y, z] = super::clamp_query(pos);
            let cell = [x, y, z].map(|v| v.div_euclid(16));
            let cube = cubes.entry(cell).or_insert_with(|| {
                crate::section_memo::terrain_cube(
                    surface,
                    caves,
                    SectionPos::new(cell[0], cell[1], cell[2]),
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
