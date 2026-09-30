use super::*;
use petramond_world::block::Block;
use petramond_world::block_state::SlabState;
use petramond_world::chunk::{Chunk, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_SIZE, SKY_FULL};
use petramond_world::facing::Facing;
use petramond_world::mathh::IVec3;
use petramond_world::section::Section;

fn shade_idx(v: &Vertex) -> u32 {
    (v.packed >> super::vertex::SHADE_SHIFT) & 0x3
}

fn light6(v: &Vertex) -> u32 {
    (v.packed >> super::vertex::SKY_SHIFT) & 0x3F
}

fn ao_idx(v: &Vertex) -> u32 {
    (v.packed >> super::vertex::AO_SHIFT) & 0x3
}

fn tile_idx(v: &Vertex) -> u32 {
    v.packed & super::vertex::TILE_MASK
}

fn uv_mode(v: &Vertex) -> u32 {
    (v.packed >> super::vertex::UV_MODE_SHIFT) & 0x7
}

fn cell_uv16(v: &Vertex) -> (u32, u32) {
    ((v.packed2 >> 6) & 0x1F, (v.packed2 >> 11) & 0x1F)
}

fn vert_at(verts: &[Vertex], shade: u32, pos: [f32; 3]) -> &Vertex {
    verts
        .iter()
        .find(|v| {
            shade_idx(v) == shade
                && v.pos
                    .iter()
                    .zip(pos.iter())
                    .all(|(a, b)| (a - b).abs() < 1e-3)
        })
        .unwrap_or_else(|| panic!("no face-kind-{shade} vertex at {pos:?}"))
}

fn in_section(wx: i32, wy: i32, wz: i32) -> bool {
    let r = 0..SECTION_SIZE as i32;
    r.contains(&wx) && r.contains(&wy) && r.contains(&wz)
}

pub(super) fn refined(section: &Section) -> Section {
    struct Nb<'a>(&'a Section);
    impl petramond_world::block::ShapeNeighborhood for Nb<'_> {
        fn block(&self, p: IVec3) -> Block {
            if in_section(p.x, p.y, p.z) {
                Block::from_id(self.0.block_raw(p.x as usize, p.y as usize, p.z as usize))
            } else {
                Block::Air
            }
        }
        fn shape_state(&self, p: IVec3) -> petramond_world::block::ShapeState {
            if in_section(p.x, p.y, p.z) {
                self.0.cell_state(p.x as usize, p.y as usize, p.z as usize)
            } else {
                petramond_world::block::ShapeState::NONE
            }
        }
    }
    let mut out = section.clone();
    for _ in 0..3 {
        let mut writes = Vec::new();
        {
            let nb = Nb(&out);
            for idx in 0..petramond_world::chunk::SECTION_VOLUME {
                let (lx, ly, lz) = petramond_world::chunk::section_local(idx);
                let block = Block::from_id(out.block_raw(lx, ly, lz));
                let k = block.shape_kind_def();
                if !k.refines {
                    continue;
                }
                let pos = IVec3::new(lx as i32, ly as i32, lz as i32);
                let cur = out.cell_state(lx, ly, lz);
                let next = k.sim.refine_state(&k.params, &nb, pos, block, cur);
                if next != cur {
                    writes.push(((lx, ly, lz), next));
                }
            }
        }
        if writes.is_empty() {
            break;
        }
        for ((lx, ly, lz), state) in writes {
            out.set_cell_state(lx, ly, lz, state);
        }
    }
    out
}

fn section_with(blocks: &[((usize, usize, usize), Block)]) -> Section {
    let mut section = Section::new(0, 0, 0);
    for &((x, y, z), b) in blocks {
        section.set_block(x, y, z, b);
    }
    section
}

fn floor_section(block: Block) -> Section {
    let mut section = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, block);
        }
    }
    section
}

fn test_rules() -> &'static petramond_world::texture_transition::Rules {
    static RULES: std::sync::LazyLock<petramond_world::texture_transition::Rules> =
        std::sync::LazyLock::new(|| {
            petramond_world::texture_transition::Rules::from_layers(&[r#"{
                "sets": [{"set": "test:organic", "mask": "organic_transition_mask", "width_texels": 6}],
                "pairs": [
                    {"pair": "test:dirt_grass", "set": "test:organic", "blocks": ["petramond:dirt", "petramond:grass"]},
                    {"pair": "test:grass_sand", "set": "test:organic", "blocks": ["petramond:grass", "petramond:sand"]}
                ]}"#])
            .expect("synthetic transition policy")
        });
    &RULES
}

fn test_ctx() -> crate::MeshContext<'static> {
    crate::MeshContext {
        content: petramond_world::content::Content::current(),
        registry: crate::MeshRegistry::global(),
        rules: test_rules(),
    }
}

fn mesh_with(
    section: &Section,
    sky: impl Fn(i32, i32, i32) -> u8,
    loaded: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    mesh_lit(
        section,
        sky,
        |_, _, _| petramond_world::light::LightRgb::ZERO,
        loaded,
    )
}

fn mesh_lit(
    section: &Section,
    sky: impl Fn(i32, i32, i32) -> u8,
    block_light: impl Fn(i32, i32, i32) -> petramond_world::light::LightRgb,
    loaded: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    let section = &refined(section);
    let dyed = section.cell_tint_map();
    build_section_mesh(
        section,
        SectionPos::new(0, 0, 0),
        test_ctx(),
        &crate::WorldReads {
            block: &|wx, wy, wz| {
                if in_section(wx, wy, wz) {
                    section.block_raw(wx as usize, wy as usize, wz as usize)
                } else {
                    Block::Air.id()
                }
            },
            cell_state: &|wx, wy, wz| {
                if in_section(wx, wy, wz) {
                    section.cell_state(wx as usize, wy as usize, wz as usize)
                } else {
                    petramond_world::block::ShapeState::NONE
                }
            },
            fluid_meta: &|wx, wy, wz| {
                if in_section(wx, wy, wz) {
                    section.fluid_meta(wx as usize, wy as usize, wz as usize)
                } else {
                    0
                }
            },
            biome: &|_, _| 0,
            skylight: &sky,
            blocklight: &block_light,
            loaded: &loaded,
            dyed: &|wx, wy, wz| {
                in_section(wx, wy, wz)
                    && dyed.contains_key(
                        &(petramond_world::chunk::section_idx(wx as usize, wy as usize, wz as usize)
                            as u16),
                    )
            },
        },
    )
}

fn mesh(section: &Section) -> ChunkMesh {
    mesh_with(section, |_, _, _| SKY_FULL, |_, _, _| true)
}

fn mesh_per_face(section: &Section) -> ChunkMesh {
    let section = &refined(section);
    fixtures::standalone(section).mesh_per_face(section, SectionPos::new(0, 0, 0))
}

fn mesh_with_sky(section: &Section, sky: impl Fn(i32, i32, i32) -> u8) -> ChunkMesh {
    mesh_with(section, sky, |_, _, _| true)
}

fn mesh_in_scene(
    section: &Section,
    pos: SectionPos,
    block: impl Fn(i32, i32, i32) -> u16,
    sky: impl Fn(i32, i32, i32) -> u8,
) -> ChunkMesh {
    build_section_mesh(
        section,
        pos,
        test_ctx(),
        &crate::WorldReads {
            block: &block,
            cell_state: &|_, _, _| petramond_world::block::ShapeState::NONE,
            fluid_meta: &|_, _, _| 0,
            biome: &|_, _| 0,
            skylight: &sky,
            blocklight: &|_, _, _| petramond_world::light::LightRgb::ZERO,
            loaded: &|_, _, _| true,
            dyed: &|_, _, _| false,
        },
    )
}

fn mesh_stairs(
    blocks: &[((usize, usize, usize), Block)],
    facings: &[((usize, usize, usize), Facing)],
) -> ChunkMesh {
    let mut section = section_with(blocks);
    for &((x, y, z), f) in facings {
        section.set_stair_facing(x, y, z, f);
    }
    mesh(&section)
}

struct TestSky {
    band: Box<[u8]>,
    ylo: i32,
    yhi: i32,
}

impl TestSky {
    fn at(&self, x: i32, y: i32, z: i32) -> u8 {
        if y > self.yhi {
            return SKY_FULL;
        }
        if y < self.ylo {
            return 0;
        }
        let ay = y - self.ylo;
        self.band[((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize]
    }
}

fn solo_skylight(c: &Chunk) -> TestSky {
    let (band, ylo, yhi) = compute_chunk_skylight(c);
    TestSky { band, ylo, yhi }
}

fn fill_chunk_layers(c: &mut Chunk, ys: std::ops::RangeInclusive<usize>, block: Block) {
    for y in ys {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                c.set_block(x, y, z, block);
            }
        }
    }
}

fn floored_chunk() -> Chunk {
    let mut c = Chunk::new(0, 0);
    fill_chunk_layers(&mut c, 0..=4, Block::Stone);
    c
}

fn walled_shaft(fill: Block) -> Chunk {
    let mut c = Chunk::new(0, 0);
    fill_chunk_layers(&mut c, 0..=0, Block::Stone);
    for y in 1..=8 {
        c.set_block(8, y, 8, fill);
        for (x, z) in [(7, 8), (9, 8), (8, 7), (8, 9)] {
            c.set_block(x, y, z, Block::Stone);
        }
    }
    c
}

fn roof_with_open_shaft(roof: Block) -> Chunk {
    let mut c = floored_chunk();
    fill_chunk_layers(&mut c, 10..=10, roof);
    c.set_block(8, 10, 8, Block::Air);
    c
}

mod ao;
mod block_light_color;
mod boxes;
mod contact;
mod equivalence;
mod fence;
mod fixtures;
mod fluid;
mod foliage;
mod glass;
mod greedy;
mod mesh_space;
mod oriented_blocks;
mod quad_streams;
mod seams;
mod skylight;
mod slabs;
mod snow;
mod stairs;
mod synthetic_fluids;
mod tint;

mod texture_transitions;
