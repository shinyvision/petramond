use super::*;
use crate::block_state::SlabSplit;

/// Parallel mesh building (the mesh pool on native) must produce byte-identical
/// meshes to a serial build: `build_section_mesh` is a pure function of
/// (section, neighbour reads) whose only shared state is the per-thread greedy
/// scratch, so rayon may only reorder independent work.
mod parallel_parity_tests {
    use super::*;
    use crate::chunk::{Chunk, SectionPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};
    use crate::section::Section;
    use crate::worldgen::generate_chunk;
    use rayon::prelude::*;
    use std::collections::HashMap;

    /// The skylight bake may run on worker/rayon threads in tools and tests, so
    /// it must be deterministic: same blocks -> byte-identical band, regardless
    /// of thread or repetition (guards the per-thread `SKY_SCRATCH` being fully
    /// reset each call and the flood being order-independent).
    #[test]
    fn skylight_bake_is_deterministic_serial_vs_parallel() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> = (-2..=2)
            .flat_map(|cz| (-2..=2).map(move |cx| (cx, cz)))
            .collect();
        let chunks: Vec<Chunk> = coords
            .iter()
            .map(|&(cx, cz)| generate_chunk(seed, cx, cz))
            .collect();

        let serial: Vec<(Box<[u8]>, i32, i32)> =
            chunks.iter().map(compute_chunk_skylight).collect();

        // Same chunk baked twice back-to-back on one thread -> identical (scratch reset).
        for (c, s) in chunks.iter().zip(&serial) {
            let again = compute_chunk_skylight(c);
            assert_eq!(&again.0[..], &s.0[..]);
            assert_eq!((again.1, again.2), (s.1, s.2));
        }

        // Parallel bake (mirrors World::poll) -> byte-identical to serial.
        let parallel: Vec<(Box<[u8]>, i32, i32)> =
            chunks.par_iter().map(compute_chunk_skylight).collect();
        for (p, s) in parallel.iter().zip(&serial) {
            assert_eq!(
                &p.0[..],
                &s.0[..],
                "parallel skylight bake differs from serial"
            );
            assert_eq!((p.1, p.2), (s.1, s.2));
        }
    }

    #[test]
    fn parallel_meshing_is_byte_identical_to_serial() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> = (-2..=2)
            .flat_map(|cz| (-2..=2).map(move |cx| (cx, cz)))
            .collect();

        // Generated columns + their baked skylight bands, the light source for
        // every section meshed below.
        struct LitColumn {
            chunk: Chunk,
            band: Box<[u8]>,
            ylo: i32,
            yhi: i32,
        }
        impl LitColumn {
            fn sky(&self, x: usize, y: i32, z: usize) -> u8 {
                if y > self.yhi {
                    return SKY_FULL;
                }
                if y < self.ylo {
                    return 0;
                }
                let ay = y - self.ylo;
                self.band
                    [((ay * CHUNK_SZ as i32 + z as i32) * CHUNK_SX as i32 + x as i32) as usize]
            }
        }
        let columns: HashMap<(i32, i32), LitColumn> = coords
            .iter()
            .map(|&(cx, cz)| {
                let chunk = generate_chunk(seed, cx, cz);
                let (band, ylo, yhi) = compute_chunk_skylight(&chunk);
                ((cx, cz), LitColumn { chunk, band, ylo, yhi })
            })
            .collect();

        // Split every generated column into its surface sections — the unit the
        // live mesh pool builds.
        let sections: Vec<(SectionPos, Section)> = coords
            .iter()
            .flat_map(|&(cx, cz)| {
                let (_, secs) =
                    crate::world::split_generated_column(&columns[&(cx, cz)].chunk);
                secs.into_iter()
                    .filter(|(cy, _)| *cy >= 0)
                    .map(move |(cy, s)| (SectionPos::new(cx, cy, cz), s))
            })
            .collect();

        let mesh_one = |item: &(SectionPos, Section)| -> ChunkMesh {
            let (pos, section) = item;
            let nb = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 {
                    return 0;
                }
                match columns.get(&(wx >> 4, wz >> 4)) {
                    Some(lc) => {
                        lc.chunk
                            .block_raw((wx & 15) as usize, wy as usize, (wz & 15) as usize)
                    }
                    None => 0,
                }
            };
            let nb_biome = |wx: i32, wz: i32| -> u8 {
                match columns.get(&(wx >> 4, wz >> 4)) {
                    Some(lc) => lc.chunk.biome_at((wx & 15) as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            let nb_light = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 {
                    return 0;
                }
                if wy >= CHUNK_SY as i32 {
                    return SKY_FULL;
                }
                match columns.get(&(wx >> 4, wz >> 4)) {
                    Some(lc) => lc.sky((wx & 15) as usize, wy, (wz & 15) as usize),
                    None => SKY_FULL,
                }
            };
            build_section_mesh(
                section,
                *pos,
                nb,
                |_, _, _| StairState::default(),
                |_, _, _| SlabState::EMPTY,
                |_, _, _| 0,
                nb_biome,
                nb_light,
                |_, _, _| 0,
                |_, _, _| true,
            )
        };

        let serial: Vec<ChunkMesh> = sections.iter().map(mesh_one).collect();
        let parallel: Vec<ChunkMesh> = sections.par_iter().map(mesh_one).collect();

        for (s, p) in serial.iter().zip(&parallel) {
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.opaque),
                bytemuck::cast_slice::<Vertex, u8>(&p.opaque),
            );
            assert_eq!(s.opaque_idx, p.opaque_idx);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.transparent),
                bytemuck::cast_slice::<Vertex, u8>(&p.transparent),
            );
            assert_eq!(s.transparent_idx, p.transparent_idx);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.far_opaque),
                bytemuck::cast_slice::<Vertex, u8>(&p.far_opaque),
            );
            assert_eq!(s.far_opaque_idx, p.far_opaque_idx);
        }
    }
}

#[test]
fn pad_local_section_mesher_matches_closure_mesher() {
    use crate::furnace::Furnace;

    const PAD: usize = SECTION_SIZE + 2;
    const PAD_VOL: usize = PAD * PAD * PAD;
    const BIOME_PAD_RADIUS: i32 = 2;
    const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);
    let pidx = |x: usize, y: usize, z: usize| (y * PAD + z) * PAD + x;
    let bidx = |x: usize, z: usize| z * BIOME_PAD + x;

    let pos = SectionPos::new(0, 0, 0);
    let mut section = floor_section(Block::Stone);
    section.set_block(2, 1, 2, Block::Grass);
    section.set_block(3, 1, 2, Block::OakLeaves);
    section.set_block(4, 1, 2, Block::ShortGrass);
    section.set_water(5, 1, 2, Block::Water, 4);
    // A BURNING furnace is the `furnace_lit` row; the machine state rides
    // along as it does in the live world (the mesher only reads the row).
    section.set_block(6, 1, 2, Block::FurnaceLit);
    section.insert_furnace(
        6,
        1,
        2,
        Furnace {
            burn_remaining: 10,
            ..Default::default()
        },
    );
    section.insert_entity_facing(6, 1, 2, Facing::East);
    section.set_block(7, 1, 2, Block::Cactus);
    section.set_block(8, 1, 2, Block::OakStairs);
    section.set_stair_facing(8, 1, 2, Facing::South);
    // Slabs: single layer, same-material full stack (cube fast path), mixed full
    // stack, and an opaque cube against the mixed stack (both-way culling).
    section.set_block(9, 1, 2, Block::OakSlab);
    section.set_slab_state(9, 1, 2, SlabState::single(SlabSplit::Y, 0, Block::OakSlab));
    section.set_block(10, 1, 2, Block::StoneSlab);
    section.set_slab_state(
        10,
        1,
        2,
        SlabState {
            split: SlabSplit::Y,
            layers: [Block::StoneSlab, Block::StoneSlab],
        },
    );
    section.set_block(11, 1, 2, Block::StoneSlab);
    section.set_slab_state(
        11,
        1,
        2,
        SlabState {
            split: SlabSplit::Y,
            layers: [Block::DirtSlab, Block::StoneSlab],
        },
    );
    section.set_block(12, 1, 2, Block::Stone);
    // Glass pair (same-block cull on the per-face path) and a connected pane run.
    section.set_block(13, 1, 2, Block::Glass);
    section.set_block(14, 1, 2, Block::Glass);
    section.set_block(2, 1, 4, Block::GlassPane);
    section.set_block(3, 1, 4, Block::GlassPane);

    let block_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if in_section(wx, wy, wz) {
            section.block_raw(wx as usize, wy as usize, wz as usize)
        } else if wy == 0
            && (-1..=SECTION_SIZE as i32).contains(&wx)
            && (-1..=SECTION_SIZE as i32).contains(&wz)
        {
            Block::Stone.id()
        } else {
            Block::Air.id()
        }
    };
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if in_section(wx, wy, wz) {
            section.water_meta(wx as usize, wy as usize, wz as usize)
        } else {
            0
        }
    };
    let stair_at = |wx: i32, wy: i32, wz: i32| -> StairState {
        if in_section(wx, wy, wz) {
            section.stair_state(wx as usize, wy as usize, wz as usize)
        } else {
            StairState::default()
        }
    };
    let slab_at = |wx: i32, wy: i32, wz: i32| -> SlabState {
        if in_section(wx, wy, wz) {
            section.slab_state(wx as usize, wy as usize, wz as usize)
        } else {
            SlabState::EMPTY
        }
    };
    let sky_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if wy < 0 {
            0
        } else if wy >= SECTION_SIZE as i32 {
            SKY_FULL
        } else {
            (18 + (wx * 3 + wy * 5 + wz * 7).rem_euclid(13)) as u8
        }
    };
    let blocklight_at =
        |wx: i32, wy: i32, wz: i32| -> u8 { ((wx + wy * 2 + wz * 3).rem_euclid(5) * 2) as u8 };
    let biome_at = |_: i32, _: i32| -> u8 { 0 };
    let loaded_at = |_: i32, _: i32, _: i32| -> bool { true };

    let serial = build_section_mesh(
        &section,
        pos,
        block_at,
        stair_at,
        slab_at,
        water_at,
        biome_at,
        sky_at,
        blocklight_at,
        loaded_at,
    );

    let mut blocks = vec![0u8; PAD_VOL];
    let mut water = vec![0u8; PAD_VOL];
    let mut skylight = vec![SKY_FULL; PAD_VOL];
    let mut blocklight = vec![0u8; PAD_VOL];
    let mut stair_states = vec![StairState::default().encode(); PAD_VOL];
    let mut slab_states = vec![SlabState::EMPTY; PAD_VOL];
    let loaded = vec![true; PAD_VOL];
    for py in 0..PAD {
        for pz in 0..PAD {
            for px in 0..PAD {
                let (wx, wy, wz) = (px as i32 - 1, py as i32 - 1, pz as i32 - 1);
                let i = pidx(px, py, pz);
                blocks[i] = block_at(wx, wy, wz);
                water[i] = water_at(wx, wy, wz);
                skylight[i] = sky_at(wx, wy, wz);
                blocklight[i] = blocklight_at(wx, wy, wz);
                stair_states[i] = stair_at(wx, wy, wz).encode();
                slab_states[i] = slab_at(wx, wy, wz);
            }
        }
    }
    let mut biome = vec![0u8; BIOME_PAD * BIOME_PAD];
    for pz in 0..BIOME_PAD {
        for px in 0..BIOME_PAD {
            biome[bidx(px, pz)] =
                biome_at(px as i32 - BIOME_PAD_RADIUS, pz as i32 - BIOME_PAD_RADIUS);
        }
    }

    let pad = build_section_mesh_from_pad(
        &section,
        pos,
        SectionMeshPad {
            blocks: &blocks,
            water: &water,
            skylight: &skylight,
            blocklight: &blocklight,
            stair_states: &stair_states,
            slab_states: &slab_states,
            loaded: &loaded,
            biome: &biome,
        },
    );

    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&serial.opaque),
        bytemuck::cast_slice::<Vertex, u8>(&pad.opaque)
    );
    assert_eq!(serial.opaque_idx, pad.opaque_idx);
    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&serial.transparent),
        bytemuck::cast_slice::<Vertex, u8>(&pad.transparent)
    );
    assert_eq!(serial.transparent_idx, pad.transparent_idx);
    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&serial.far_opaque),
        bytemuck::cast_slice::<Vertex, u8>(&pad.far_opaque)
    );
    assert_eq!(serial.far_opaque_idx, pad.far_opaque_idx);
    assert_eq!(
        bytemuck::cast_slice::<ModelVertex, u8>(&serial.model),
        bytemuck::cast_slice::<ModelVertex, u8>(&pad.model)
    );
    assert_eq!(serial.model_idx, pad.model_idx);
    assert_eq!(serial.mesh_dirty, pad.mesh_dirty);
}
