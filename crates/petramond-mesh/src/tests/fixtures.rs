//! World-coordinate scene fixtures shared by the snapshot and parity suites:
//! a [`Scene`] answers every neighbour read a section mesh makes, and
//! [`Scene::mesh`] meshes one section of it through the production build.

use super::*;
use crate::builder::{build_section_mesh_with, WorldReads};
use petramond_world::block::ShapeState;
use petramond_world::block_state::SlabSplit;
use petramond_world::chunk::CHUNK_SY;
use petramond_world::light::LightRgb;
use std::collections::HashMap;
use std::rc::Rc;

type Reads<T> = Box<dyn Fn(i32, i32, i32) -> T>;

/// Every world read a section mesh depends on, by world coordinate.
pub(super) struct Scene {
    pub block: Reads<u16>,
    pub cell_state: Reads<ShapeState>,
    pub fluid: Reads<u8>,
    pub biome: Box<dyn Fn(i32, i32) -> u8>,
    pub sky: Reads<u8>,
    pub blocklight: Reads<LightRgb>,
    pub loaded: Reads<bool>,
    pub dyed: Reads<bool>,
}

impl Scene {
    fn reads(&self) -> WorldReads<'_> {
        WorldReads {
            block: &self.block,
            cell_state: &self.cell_state,
            fluid_meta: &self.fluid,
            biome: &self.biome,
            skylight: &self.sky,
            blocklight: &self.blocklight,
            loaded: &self.loaded,
            dyed: &self.dyed,
        }
    }

    /// Mesh the section at `pos` of this scene through the production build:
    /// its one-cell pad is sampled from the scene's reads, then meshed with
    /// the exposure-mask fast path.
    pub(super) fn mesh(&self, section: &Section, pos: SectionPos) -> ChunkMesh {
        build_section_mesh_with(section, pos, test_rules(), &self.reads(), true)
    }

    /// [`Self::mesh`] with every cube face culled by asking its front cell —
    /// the reference the fast path must reproduce byte for byte.
    pub(super) fn mesh_per_face(&self, section: &Section, pos: SectionPos) -> ChunkMesh {
        build_section_mesh_with(section, pos, test_rules(), &self.reads(), false)
    }
}

/// `section` alone at the origin: air around it, full sky, all loaded, its
/// own tint map as the dye exclusions.
pub(super) fn standalone(section: &Section) -> Scene {
    let s = Rc::new(section.clone());
    let (b, c, f, d) = (s.clone(), s.clone(), s.clone(), s);
    let at = |wx: i32, wy: i32, wz: i32| {
        in_section(wx, wy, wz).then_some((wx as usize, wy as usize, wz as usize))
    };
    Scene {
        block: Box::new(move |x, y, z| {
            at(x, y, z).map_or(Block::Air.id(), |(x, y, z)| b.block_raw(x, y, z))
        }),
        cell_state: Box::new(move |x, y, z| {
            at(x, y, z).map_or(ShapeState::NONE, |(x, y, z)| c.cell_state(x, y, z))
        }),
        fluid: Box::new(move |x, y, z| at(x, y, z).map_or(0, |(x, y, z)| f.fluid_meta(x, y, z))),
        biome: Box::new(|_, _| 0),
        sky: Box::new(|_, _, _| SKY_FULL),
        blocklight: Box::new(|_, _, _| LightRgb::ZERO),
        loaded: Box::new(|_, _, _| true),
        dyed: Box::new(move |x, y, z| {
            at(x, y, z).is_some_and(|(x, y, z)| {
                d.cell_tint_map()
                    .contains_key(&(petramond_world::chunk::section_idx(x, y, z) as u16))
            })
        }),
    }
}

/// The hand-built showcase: one of nearly every cube-path feature on a stone
/// floor (grass, leaves, a plant, water reaching the pad's top face, a lit
/// furnace, cactus, stairs, every slab stacking, glass, panes, snow over a
/// cube and over the floor, transitions across the section seam with a dyed
/// donor), under patterned coloured light.
pub(super) fn showcase() -> (Section, Scene) {
    use petramond_world::furnace::Furnace;
    let mut section = floor_section(Block::Stone);
    section.set_block(2, 1, 2, Block::Grass);
    section.set_block(3, 1, 2, Block::OakLeaves);
    section.set_block(4, 1, 2, Block::ShortGrass);
    section.set_fluid(5, 1, 2, Block::Water, 4);
    // Top-of-section water: the pad-local fill probe reads the neighbour
    // ABOVE (one past the top pad face for that neighbour) — must not OOB.
    section.set_fluid(5, SECTION_SIZE - 1, 2, Block::Water, 0);
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
    // Snow layers over a full cube (grass: snowy sides + culled top) and over
    // the bare floor — the floor-flush SEAL cull of the exposure masks.
    section.set_block(2, 2, 2, Block::SnowLayer);
    section.set_block(12, 1, 4, Block::SnowLayer);

    section.set_block(7, 1, 7, Block::Dirt);
    section.set_block(8, 1, 7, Block::Grass);
    section.set_block(8, 2, 7, Block::ShortGrass);
    section.set_block(8, 1, 8, Block::Sand);
    section.set_block(8, 2, 8, Block::PebblesSmall);
    // Transitions across the section seam: a dyed neighbour cell excludes
    // itself as a donor, an undyed one does not.
    section.set_block(15, 1, 3, Block::Dirt);
    section.set_block(15, 1, 5, Block::Dirt);

    // Resolve stored shape states (stair corners, pane masks) the way the
    // world's edit cascade would have — the fixture wrote raw cells.
    let section = refined(&section);
    let s = Rc::new(section.clone());
    let (b, c, f) = (s.clone(), s.clone(), s);
    let n = SECTION_SIZE as i32;
    let scene = Scene {
        block: Box::new(move |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                b.block_raw(wx as usize, wy as usize, wz as usize)
            } else if wy == 0 && (-1..=n).contains(&wx) && (-1..=n).contains(&wz) {
                Block::Stone.id()
            } else if wy == 1 && wx == n && (wz == 3 || wz == 5) {
                // Grass donors in the east neighbour; the one at z = 3 is dyed.
                Block::Grass.id()
            } else if wy == n && wx == 5 && wz == 2 {
                // Water column continuing above the section (pad top face).
                Block::Water.id()
            } else {
                Block::Air.id()
            }
        }),
        cell_state: Box::new(move |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                c.cell_state(wx as usize, wy as usize, wz as usize)
            } else {
                ShapeState::NONE
            }
        }),
        fluid: Box::new(move |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                f.fluid_meta(wx as usize, wy as usize, wz as usize)
            } else {
                0
            }
        }),
        biome: Box::new(|_, _| 0),
        sky: Box::new(move |wx, wy, wz| {
            if wy < 0 {
                0
            } else if wy >= n {
                SKY_FULL
            } else {
                (18 + (wx * 3 + wy * 5 + wz * 7).rem_euclid(13)) as u8
            }
        }),
        // A COLOURED per-cell light, so every channel of the gather is pinned,
        // not just brightness.
        blocklight: Box::new(|wx, wy, wz| {
            LightRgb::new(
                ((wx + wy * 2 + wz * 3).rem_euclid(5) * 2) as u8,
                ((wx * 2 + wy + wz * 5).rem_euclid(7) * 2) as u8,
                ((wx * 3 + wy * 7 + wz).rem_euclid(4) * 2) as u8,
            )
        }),
        loaded: Box::new(|_, _, _| true),
        dyed: Box::new(move |wx, wy, wz| (wx, wy, wz) == (n, 1, 3)),
    };
    (section, scene)
}

/// The generated columns around the origin with their baked skylight: every
/// surface section of the centre column, each with the neighbour columns
/// answering its pad reads.
pub(super) fn generated_sections() -> Vec<(SectionPos, Section, Scene)> {
    struct LitColumn {
        chunk: Chunk,
        band: Box<[u8]>,
        ylo: i32,
        yhi: i32,
    }
    let seed = 0x1234_5678u32;
    let columns: Rc<HashMap<(i32, i32), LitColumn>> = Rc::new(
        (-1..=1)
            .flat_map(|cz| (-1..=1).map(move |cx| (cx, cz)))
            .map(|(cx, cz)| {
                let chunk = petramond_worldgen::generate_chunk(seed, cx, cz);
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
            .collect(),
    );
    let (_, sections) =
        petramond_world::column_split::split_generated_column(&columns[&(0, 0)].chunk);
    sections
        .into_iter()
        .filter(|(cy, _)| *cy >= 0)
        .map(|(cy, section)| {
            let (b, f, g, s, l) = (
                columns.clone(),
                columns.clone(),
                columns.clone(),
                columns.clone(),
                columns.clone(),
            );
            let in_height = |wy: i32| (0..CHUNK_SY as i32).contains(&wy);
            let scene = Scene {
                block: Box::new(move |wx, wy, wz| match b.get(&(wx >> 4, wz >> 4)) {
                    Some(lc) if in_height(wy) => {
                        lc.chunk
                            .block_raw((wx & 15) as usize, wy as usize, (wz & 15) as usize)
                    }
                    _ => 0,
                }),
                cell_state: Box::new(|_, _, _| ShapeState::NONE),
                fluid: Box::new(move |wx, wy, wz| match f.get(&(wx >> 4, wz >> 4)) {
                    Some(lc) if in_height(wy) => {
                        lc.chunk
                            .fluid_meta((wx & 15) as usize, wy as usize, (wz & 15) as usize)
                    }
                    _ => 0,
                }),
                biome: Box::new(move |wx, wz| {
                    g.get(&(wx >> 4, wz >> 4))
                        .map_or(0, |lc| lc.chunk.biome_at((wx & 15) as usize, (wz & 15) as usize))
                }),
                sky: Box::new(move |wx, wy, wz| {
                    if wy < 0 {
                        return 0;
                    }
                    match s.get(&(wx >> 4, wz >> 4)) {
                        Some(lc) if wy <= lc.yhi && wy >= lc.ylo => {
                            let ay = wy - lc.ylo;
                            lc.band[((ay * CHUNK_SZ as i32 + (wz & 15)) * CHUNK_SX as i32
                                + (wx & 15)) as usize]
                        }
                        Some(lc) if wy < lc.ylo => 0,
                        _ => SKY_FULL,
                    }
                }),
                blocklight: Box::new(|_, _, _| LightRgb::ZERO),
                loaded: Box::new(move |wx, _, wz| l.contains_key(&(wx >> 4, wz >> 4))),
                dyed: Box::new(|_, _, _| false),
            };
            (SectionPos::new(0, cy, 0), section, scene)
        })
        .collect()
}

/// Every registered block (air aside) on a lattice over a stone floor, as
/// many sections as the registry needs: each render family's geometry for
/// its default placement, refined as the world would store it.
pub(super) fn catalog_sections() -> Vec<Section> {
    let slots: Vec<(usize, usize, usize)> = (1..SECTION_SIZE)
        .step_by(2)
        .flat_map(|y| {
            (1..SECTION_SIZE).step_by(2).flat_map(move |z| {
                (1..SECTION_SIZE).step_by(2).map(move |x| (x, y, z))
            })
        })
        .collect();
    let blocks: Vec<Block> = Block::all()
        .iter()
        .copied()
        .filter(|&b| b != Block::Air)
        .collect();
    blocks
        .chunks(slots.len())
        .map(|chunk| {
            let mut section = floor_section(Block::Stone);
            for (&(x, y, z), &block) in slots.iter().zip(chunk) {
                section.set_block(x, y, z, block);
            }
            refined(&section)
        })
        .collect()
}
