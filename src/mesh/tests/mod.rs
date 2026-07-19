use super::*;
use crate::block::Block;
use crate::block_state::{SlabState, StairState};
use crate::chunk::{Chunk, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_SIZE, SKY_FULL};
use crate::facing::Facing;
use crate::section::Section;

// --- Fixtures ---------------------------------------------------------------
//
// Packed-vertex decoders, scene builders, and `build_section_mesh` wrappers
// shared by the tests below. The wrappers answer every voxel lookup from the
// section itself (air / no water / default stair+slab state outside it);
// skylight and loadedness default to uniform full sky and everything-loaded
// unless a test overrides them.

fn shade_idx(v: &Vertex) -> u32 {
    (v.packed >> 10) & 0x3
}

/// Word 1's light bits (23..29) — SKY-ONLY since the channel split; every scene
/// in these tests is sky-lit (no emitters), so it also equals the total light.
fn light6(v: &Vertex) -> u32 {
    (v.packed >> 23) & 0x3F
}

/// AO bits (word 1, 21..23).
fn ao_idx(v: &Vertex) -> u32 {
    (v.packed >> 21) & 0x3
}

/// Tile id bits (word 1, 0..8).
fn tile_idx(v: &Vertex) -> u32 {
    v.packed & 0xFF
}

fn uv_mode(v: &Vertex) -> u32 {
    (v.packed >> super::vertex::UV_MODE_SHIFT) & 0x7
}

/// The cell-local UV bits (packed2 6..11 / 11..16), only meaningful on
/// UV_MODE_CELL_LOCAL vertices.
fn cell_uv16(v: &Vertex) -> (u32, u32) {
    ((v.packed2 >> 6) & 0x1F, (v.packed2 >> 11) & 0x1F)
}

/// The vertex of face kind `shade` sitting exactly at `pos`, or panic.
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

/// A section at (0, 0, 0) with the given blocks set on empty air.
fn section_with(blocks: &[((usize, usize, usize), Block)]) -> Section {
    let mut section = Section::new(0, 0, 0);
    for &((x, y, z), b) in blocks {
        section.set_block(x, y, z, b);
    }
    section
}

/// A section whose whole y=0 layer is `block` — a flat 16×16 floor.
fn floor_section(block: Block) -> Section {
    let mut section = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, block);
        }
    }
    section
}

/// Mesh `section` standalone with overridable skylight and loadedness; all
/// other lookups answer from the section itself.
fn mesh_with(
    section: &Section,
    sky: impl Fn(i32, i32, i32) -> u8,
    loaded: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    build_section_mesh(
        section,
        SectionPos::new(0, 0, 0),
        |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                section.block_raw(wx as usize, wy as usize, wz as usize)
            } else {
                Block::Air.id()
            }
        },
        |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                section.stair_state(wx as usize, wy as usize, wz as usize)
            } else {
                StairState::default()
            }
        },
        |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                section.slab_state(wx as usize, wy as usize, wz as usize)
            } else {
                SlabState::EMPTY
            }
        },
        |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                section.water_meta(wx as usize, wy as usize, wz as usize)
            } else {
                0
            }
        },
        |_, _| 0,
        sky,
        |_, _, _| 0,
        loaded,
    )
}

/// Mesh `section` standalone under uniform full skylight, everything loaded.
fn mesh(section: &Section) -> ChunkMesh {
    mesh_with(section, |_, _, _| SKY_FULL, |_, _, _| true)
}

/// Mesh `section` standalone with a custom baked-skylight shape.
fn mesh_with_sky(section: &Section, sky: impl Fn(i32, i32, i32) -> u8) -> ChunkMesh {
    mesh_with(section, sky, |_, _, _| true)
}

/// Mesh one section of a multi-section scene: blocks and skylight answer from
/// world-coordinate closures (no stairs/slabs/water anywhere, all loaded).
fn mesh_in_scene(
    section: &Section,
    pos: SectionPos,
    block: impl Fn(i32, i32, i32) -> u8,
    sky: impl Fn(i32, i32, i32) -> u8,
) -> ChunkMesh {
    build_section_mesh(
        section,
        pos,
        block,
        |_, _, _| StairState::default(),
        |_, _, _| SlabState::EMPTY,
        |_, _, _| 0,
        |_, _| 0,
        sky,
        |_, _, _| 0,
        |_, _, _| true,
    )
}

/// A section holding bottom-half stairs (plus companion blocks), meshed
/// standalone under uniform full skylight.
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

/// Sampler over a computed skylight band, for the skylight unit tests.
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

/// Fill the chunk's whole 16×16 footprint with `block` for every y in `ys`.
fn fill_chunk_layers(c: &mut Chunk, ys: std::ops::RangeInclusive<usize>, block: Block) {
    for y in ys {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                c.set_block(x, y, z, block);
            }
        }
    }
}

/// Stone floor (y=0..=4) over the whole chunk, so test columns are not open
/// below -- keeps the volumetric descent the only thing under study.
fn floored_chunk() -> Chunk {
    let mut c = Chunk::new(0, 0);
    fill_chunk_layers(&mut c, 0..=4, Block::Stone);
    c
}

/// Build an opaque-walled vertical shaft of `fill` from y=1..=8 over a floor,
/// so the only light path is straight down through `fill`.
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

/// `roof` across the whole chunk at y=10 over a floored chunk, with one open
/// shaft cell at (8, 10, 8).
fn roof_with_open_shaft(roof: Block) -> Chunk {
    let mut c = floored_chunk();
    fill_chunk_layers(&mut c, 10..=10, roof);
    c.set_block(8, 10, 8, Block::Air);
    c
}

// --- Tests ------------------------------------------------------------------

mod ao;
mod contact;
mod foliage;
mod glass;
mod greedy;
mod oriented_blocks;
mod parity;
mod seams;
mod skylight;
mod slabs;
mod stairs;
mod water;
