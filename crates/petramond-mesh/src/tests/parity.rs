use super::*;

/// Parallel mesh building (the mesh pool on native) must produce byte-identical
/// meshes to a serial build: `build_section_mesh` is a pure function of
/// (section, neighbour reads) whose only shared state is the per-thread greedy
/// scratch, so rayon may only reorder independent work.
mod parallel_parity_tests {
    use super::*;
    use petramond_world::chunk::{Chunk, SectionPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};
    use petramond_world::section::Section;
    use petramond_worldgen::generate_chunk;
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
                self.band[((ay * CHUNK_SZ as i32 + z as i32) * CHUNK_SX as i32 + x as i32) as usize]
            }
        }
        let columns: HashMap<(i32, i32), LitColumn> = coords
            .iter()
            .map(|&(cx, cz)| {
                let chunk = generate_chunk(seed, cx, cz);
                let (band, ylo, yhi) = compute_chunk_skylight(&chunk);
                (
                    (cx, cz),
                    LitColumn {
                        chunk,
                        band,
                        ylo,
                        yhi,
                    },
                )
            })
            .collect();

        // Split every generated column into its surface sections — the unit the
        // live mesh pool builds.
        let sections: Vec<(SectionPos, Section)> = coords
            .iter()
            .flat_map(|&(cx, cz)| {
                let (_, secs) = petramond_world::column_split::split_generated_column(
                    &columns[&(cx, cz)].chunk,
                );
                secs.into_iter()
                    .filter(|(cy, _)| *cy >= 0)
                    .map(move |(cy, s)| (SectionPos::new(cx, cy, cz), s))
            })
            .collect();

        let mesh_one = |item: &(SectionPos, Section)| -> ChunkMesh {
            let (pos, section) = item;
            let nb = |wx: i32, wy: i32, wz: i32| -> u16 {
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
                test_ctx(),
                &crate::WorldReads {
                    block: &nb,
                    cell_state: &|_, _, _| petramond_world::block::ShapeState::NONE,
                    fluid_meta: &|_, _, _| 0,
                    biome: &nb_biome,
                    skylight: &nb_light,
                    blocklight: &|_, _, _| petramond_world::light::LightRgb::ZERO,
                    loaded: &|_, _, _| true,
                    dyed: &|_, _, _| false,
                },
            )
        };

        let serial: Vec<ChunkMesh> = sections.iter().map(mesh_one).collect();
        let parallel: Vec<ChunkMesh> = sections.par_iter().map(mesh_one).collect();

        for (s, p) in serial.iter().zip(&parallel) {
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.opaque),
                bytemuck::cast_slice::<Vertex, u8>(&p.opaque),
            );
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.transparent),
                bytemuck::cast_slice::<Vertex, u8>(&p.transparent),
            );
            assert_eq!(s.far_opaque_len, p.far_opaque_len);
        }
    }
}

/// Every stream of two meshes, byte for byte.
fn assert_same_mesh(a: &ChunkMesh, b: &ChunkMesh, what: &str) {
    let verts = |v: &[Vertex]| bytemuck::cast_slice::<Vertex, u8>(v).to_vec();
    assert_eq!(verts(&a.opaque), verts(&b.opaque), "{what}: opaque");
    assert_eq!(a.far_opaque_len, b.far_opaque_len, "{what}: far LOD");
    assert_eq!(
        verts(&a.transparent),
        verts(&b.transparent),
        "{what}: transparent"
    );
    assert_eq!(
        verts(&a.transparent_two_sided),
        verts(&b.transparent_two_sided),
        "{what}: transparent two-sided"
    );
    assert_eq!(
        verts(&a.translucent),
        verts(&b.translucent),
        "{what}: translucent"
    );
    assert_eq!(
        bytemuck::cast_slice::<ModelVertex, u8>(&a.model),
        bytemuck::cast_slice::<ModelVertex, u8>(&b.model),
        "{what}: model"
    );
    assert_eq!(a.model_idx, b.model_idx, "{what}: model indices");
    assert_eq!(
        a.model_blend_idx, b.model_blend_idx,
        "{what}: model blend indices"
    );
    assert_eq!(
        bytemuck::cast_slice::<crate::ContactShadowVertex, u8>(&a.contact),
        bytemuck::cast_slice::<crate::ContactShadowVertex, u8>(&b.contact),
        "{what}: contact"
    );
    assert_eq!(a.mesh_dirty, b.mesh_dirty, "{what}: dirty flag");
}

/// The exposure-mask fast path (buried rows skipped, cube faces culled from
/// bitsets) and the per-face cull (every cube face asks its front cell) must
/// mesh the showcase identically — slabs, snow seals, glass, fluids at the
/// pad's top face, coloured light and cross-seam transitions included.
#[test]
fn exposure_masks_match_the_per_face_cull_on_the_showcase() {
    let (section, scene) = fixtures::showcase();
    let pos = SectionPos::new(0, 0, 0);
    let fast = scene.mesh(&section, pos);
    let transitions_at = |mesh: &ChunkMesh, z: f32| {
        mesh.opaque
            .chunks_exact(4)
            .filter(|q| {
                q.iter().all(|v| {
                    v.pos[1] == 2.0 && v.pos[0] >= 15.0 && v.pos[2] >= z && v.pos[2] <= z + 1.0
                }) && crate::vertex::transition::Transition::decode(&q[0]).is_some()
            })
            .count()
    };
    assert_eq!(
        transitions_at(&fast, 3.0),
        0,
        "a dyed cross-section donor is excluded"
    );
    assert_eq!(
        transitions_at(&fast, 5.0),
        1,
        "an undyed cross-section donor bleeds"
    );
    assert_same_mesh(&fast, &scene.mesh_per_face(&section, pos), "showcase");
}

/// The same parity over real generated terrain: surface, water, foliage and
/// the buried rows the masks exist to skip.
#[test]
fn exposure_masks_match_the_per_face_cull_on_generated_terrain() {
    for (pos, section, scene) in fixtures::generated_sections() {
        assert_same_mesh(
            &scene.mesh(&section, pos),
            &scene.mesh_per_face(&section, pos),
            &format!("generated section {pos:?}"),
        );
    }
}

/// A small deterministic generator (xorshift64*): the parity property runs
/// the same random sections on every platform and every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A stable per-cell hash for the random scene's neighbour shell.
fn cell_hash(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mixed = seed
        ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ (z as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
    Rng(mixed | 1).next()
}

/// One random section at the origin and the scene around it: a mix of air,
/// plain terrain and ANY registered block (so every render family meets
/// every other), fluids at random levels, and a neighbour shell with random
/// blocks, skylight, coloured block light, gaps in loadedness and dyed cells.
fn random_scene(seed: u64) -> (Section, fixtures::Scene) {
    let all: Vec<Block> = Block::all().to_vec();
    let common = [
        Block::Air,
        Block::Stone,
        Block::Dirt,
        Block::Grass,
        Block::Water,
        Block::OakLeaves,
        Block::Glass,
        Block::SnowLayer,
    ];
    let mut rng = Rng(seed | 1);
    let mut section = Section::new(0, 0, 0);
    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let block = match rng.below(10) {
                    0..=3 => Block::Air,
                    4..=7 => common[rng.below(common.len() as u64) as usize],
                    _ => all[rng.below(all.len() as u64) as usize],
                };
                if block.is_fluid() {
                    let level = rng.below(16) as u8;
                    let falling = if rng.below(4) == 0 {
                        petramond_world::fluid_math::FALLING
                    } else {
                        0
                    };
                    section.set_fluid(x, y, z, block, level | falling);
                } else {
                    section.set_block(x, y, z, block);
                }
            }
        }
    }
    let section = refined(&section);
    let shell = [
        Block::Air,
        Block::Air,
        Block::Stone,
        Block::Grass,
        Block::Water,
        Block::OakLeaves,
        Block::Glass,
        Block::OakSlab,
        Block::SnowLayer,
    ];
    let s = std::rc::Rc::new(section.clone());
    let (b, f) = (s.clone(), s);
    let n = SECTION_SIZE as i32;
    let scene = fixtures::Scene {
        block: Box::new(move |x, y, z| {
            if in_section(x, y, z) {
                b.block_raw(x as usize, y as usize, z as usize)
            } else {
                shell[(cell_hash(seed, x, y, z) % shell.len() as u64) as usize].id()
            }
        }),
        cell_state: Box::new(|_, _, _| petramond_world::block::ShapeState::NONE),
        fluid: Box::new(move |x, y, z| {
            if in_section(x, y, z) {
                f.fluid_meta(x as usize, y as usize, z as usize)
            } else {
                (cell_hash(seed ^ 1, x, y, z) % 8) as u8
            }
        }),
        biome: Box::new(move |x, z| (cell_hash(seed ^ 2, x, 0, z) % 4) as u8),
        sky: Box::new(move |x, y, z| {
            (cell_hash(seed ^ 3, x, y, z) % (u64::from(SKY_FULL) + 1)) as u8
        }),
        blocklight: Box::new(move |x, y, z| {
            let h = cell_hash(seed ^ 4, x, y, z);
            petramond_world::light::LightRgb::new(
                (h % 16) as u8 * 2,
                ((h >> 8) % 16) as u8 * 2,
                ((h >> 16) % 16) as u8 * 2,
            )
        }),
        loaded: Box::new(move |x, y, z| {
            (0..n).contains(&y) || cell_hash(seed ^ 5, x, y, z) % 8 != 0
        }),
        dyed: Box::new(move |x, y, z| {
            !in_section(x, y, z) && cell_hash(seed ^ 6, x, y, z) % 5 == 0
        }),
    };
    (section, scene)
}

/// Property: on random sections the fast path and the per-face cull agree
/// byte for byte on every stream.
#[test]
fn exposure_masks_match_the_per_face_cull_on_random_sections() {
    let pos = SectionPos::new(0, 0, 0);
    for seed in 0..48u64 {
        let (section, scene) = random_scene(0x5eed_0000 + seed);
        assert_same_mesh(
            &scene.mesh(&section, pos),
            &scene.mesh_per_face(&section, pos),
            &format!("random section seed {seed}"),
        );
    }
}

/// The public closure front end lowers onto the pad exactly like the live
/// mesh pool's assembler: meshing the showcase through `build_section_mesh`
/// equals meshing a pad assembled by hand from the same reads.
#[test]
fn closure_front_end_meshes_the_pad_the_mesh_pool_would_assemble() {
    const PAD: usize = SECTION_SIZE + 2;
    const PAD_VOL: usize = PAD * PAD * PAD;
    const BIOME_PAD: usize = SECTION_SIZE + 4;
    let pidx = |x: usize, y: usize, z: usize| (y * PAD + z) * PAD + x;
    let (section, scene) = fixtures::showcase();
    let pos = SectionPos::new(0, 0, 0);
    let closures = build_section_mesh(&section, pos, test_ctx(), &scene.reads());

    let mut blocks = vec![0u16; PAD_VOL];
    let mut fluid = vec![0u8; PAD_VOL];
    let mut skylight = vec![SKY_FULL; PAD_VOL];
    let mut blocklight = vec![petramond_world::light::LightRgb::ZERO; PAD_VOL];
    let mut cell_states = vec![petramond_world::block::ShapeState::NONE; PAD_VOL];
    let mut loaded = vec![false; PAD_VOL];
    let mut transition_blocked = vec![false; PAD_VOL];
    for py in 0..PAD {
        for pz in 0..PAD {
            for px in 0..PAD {
                let (wx, wy, wz) = (px as i32 - 1, py as i32 - 1, pz as i32 - 1);
                let i = pidx(px, py, pz);
                blocks[i] = (scene.block)(wx, wy, wz);
                transition_blocked[i] = (scene.dyed)(wx, wy, wz)
                    || petramond_world::block::snow_cover_at(IVec3::new(wx, wy + 1, wz), |p| {
                        Block::from_id((scene.block)(p.x, p.y, p.z))
                    })
                    .is_some();
                fluid[i] = (scene.fluid)(wx, wy, wz);
                skylight[i] = (scene.sky)(wx, wy, wz);
                blocklight[i] = (scene.blocklight)(wx, wy, wz);
                cell_states[i] = (scene.cell_state)(wx, wy, wz);
                loaded[i] = (scene.loaded)(wx, wy, wz);
            }
        }
    }
    let mut biome = vec![0u8; BIOME_PAD * BIOME_PAD];
    for pz in 0..BIOME_PAD {
        for px in 0..BIOME_PAD {
            biome[pz * BIOME_PAD + px] = (scene.biome)(px as i32 - 2, pz as i32 - 2);
        }
    }
    let pad = build_section_mesh_from_pad(
        &section,
        pos,
        SectionMeshPad {
            blocks: &blocks,
            fluid: &fluid,
            skylight: &skylight,
            blocklight: &blocklight,
            cell_states: &cell_states,
            loaded: &loaded,
            transition_blocked: &transition_blocked,
            biome: &biome,
        },
        test_ctx(),
    );
    assert_same_mesh(&closures, &pad, "closure front end vs assembled pad");
}
