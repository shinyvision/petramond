use petramond_world::chunk::{section_idx, SectionPos};
use petramond_world::section::BlockCube;
use std::collections::BTreeMap;

/// The top solid block's y of every column, read from the same memoized surface tiles column
/// generation fills, so a query and the generator share the work in either order.
pub fn heights_at(seed: u32, columns: &[[i32; 2]]) -> Vec<i32> {
    let generator = crate::driver::ChunkGenerator::shared(seed);
    let (surface, caves) = generator.sources();
    let mut hot: Vec<([i32; 2], std::sync::Arc<crate::feature::RegionTile>)> = Vec::new();
    columns
        .iter()
        .map(|&[x, z]| {
            let cell = [x.div_euclid(16), z.div_euclid(16)];
            let tile = match hot.iter().position(|(c, _)| *c == cell) {
                Some(i) => &hot[i].1,
                None => {
                    if hot.len() == 16 {
                        hot.remove(0);
                    }
                    hot.push((
                        cell,
                        crate::feature::cached_tile(surface, caves, cell[0], cell[1]),
                    ));
                    &hot[hot.len() - 1].1
                }
            };
            tile.raw()[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize]
        })
        .collect()
}

/// [`heights_at`] over the inclusive column rectangle `min..=max`, row by row along x, copied a
/// tile row at a time.
pub fn heights_in(seed: u32, min: [i32; 2], max: [i32; 2]) -> Vec<i32> {
    let generator = crate::driver::ChunkGenerator::shared(seed);
    let (surface, caves) = generator.sources();
    let width = (max[0] - min[0] + 1).max(0) as usize;
    let mut out = vec![0; width * (max[1] - min[1] + 1).max(0) as usize];
    for tz in min[1].div_euclid(16)..=max[1].div_euclid(16) {
        for tx in min[0].div_euclid(16)..=max[0].div_euclid(16) {
            let tile = crate::feature::cached_tile(surface, caves, tx, tz);
            let (x0, x1) = (min[0].max(tx * 16), max[0].min(tx * 16 + 15));
            for z in min[1].max(tz * 16)..=max[1].min(tz * 16 + 15) {
                let src = ((z - tz * 16) * 16) as usize;
                let dst = (z - min[1]) as usize * width;
                let (a, b) = ((x0 - tx * 16) as usize, (x1 - tx * 16) as usize);
                out[dst + (x0 - min[0]) as usize..=dst + (x1 - min[0]) as usize]
                    .copy_from_slice(&tile.raw()[src + a..=src + b]);
            }
        }
    }
    out
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
