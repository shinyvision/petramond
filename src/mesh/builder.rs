use std::cell::RefCell;

use glam::{IVec3, Vec3};

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
use crate::block_model::{self, BlockModelKind};
use crate::chunk::{
    section_idx, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y,
};
#[cfg(test)]
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use crate::furnace::Facing;
use crate::section::Section;
use crate::torch::{warm_amount, warm_tint};

use super::face::{cactus_quad, cross_quads, quad_for, should_flip, vertex_ao, Face, FACES};
use super::tint;
use super::vertex::{
    pack_vertex, pack_vertex2, ChunkMesh, ModelVertex, Vertex, UV_MODE_NONE, UV_MODE_SHIFT,
};
use super::water::{self, SideVsWater, WaterSurface};

/// The horizontal cube face a furnace's front points to, for its [`Facing`].
#[inline]
fn facing_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::NegZ,
        Facing::South => Face::PosZ,
        Facing::West => Face::NegX,
        Facing::East => Face::PosX,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LeafMeshMode {
    Detailed,
    Simplified,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MeshOptions {
    pub leaf_mesh_mode: LeafMeshMode,
}

impl MeshOptions {
    pub const DETAILED: Self = Self {
        leaf_mesh_mode: LeafMeshMode::Detailed,
    };

    pub const FAR_LEAVES: Self = Self {
        leaf_mesh_mode: LeafMeshMode::Simplified,
    };
}

const SECTION_PAD: usize = SECTION_SIZE + 2;
const BIOME_PAD_RADIUS: i32 = 2;
const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);
// Long greedy edges can meet subdivided neighbour faces as T-junctions; a tiny tangent-only
// overlap covers the rasterizer crack without moving the face plane or affecting water.
const GREEDY_FACE_OVERLAP: f32 = 1.0 / 1024.0;

#[inline]
fn mesh_pad_idx(x: usize, y: usize, z: usize) -> usize {
    (y * SECTION_PAD + z) * SECTION_PAD + x
}

#[inline]
fn biome_pad_idx(x: usize, z: usize) -> usize {
    z * BIOME_PAD + x
}

pub(crate) struct SectionMeshPad<'a> {
    pub blocks: &'a [u8],
    pub water: &'a [u8],
    pub skylight: &'a [u8],
    pub blocklight: &'a [u8],
    pub stair_facings: &'a [u8],
    pub loaded: &'a [bool],
    pub biome: &'a [u8],
}

impl SectionMeshPad<'_> {
    #[inline]
    fn block_at_pad(&self, px: usize, py: usize, pz: usize) -> Block {
        Block::from_id(self.blocks[mesh_pad_idx(px, py, pz)])
    }

    #[inline]
    fn world_idx(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> Option<usize> {
        let (px, py, pz) = (wx - (ox - 1), wy - (oy - 1), wz - (oz - 1));
        let n = SECTION_PAD as i32;
        if (0..n).contains(&px) && (0..n).contains(&py) && (0..n).contains(&pz) {
            Some(mesh_pad_idx(px as usize, py as usize, pz as usize))
        } else {
            None
        }
    }

    #[inline]
    fn block_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> u8 {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(0, |i| self.blocks[i])
    }

    #[inline]
    fn stair_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> Facing {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(Facing::North, |i| Facing::from_u8(self.stair_facings[i]))
    }

    #[inline]
    fn water_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> u8 {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(0, |i| self.water[i])
    }

    #[inline]
    fn skylight_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy >= WORLD_MAX_Y {
            return SKY_FULL;
        }
        if wy < WORLD_MIN_Y {
            return 0;
        }
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(SKY_FULL, |i| self.skylight[i])
    }

    #[inline]
    fn blocklight_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> u8 {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(0, |i| self.blocklight[i])
    }

    #[inline]
    fn loaded_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> bool {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .is_some_and(|i| self.loaded[i])
    }

    #[inline]
    fn biome_world(&self, ox: i32, oz: i32, wx: i32, wz: i32) -> u8 {
        let (px, pz) = (wx - (ox - BIOME_PAD_RADIUS), wz - (oz - BIOME_PAD_RADIUS));
        let n = BIOME_PAD as i32;
        if (0..n).contains(&px) && (0..n).contains(&pz) {
            self.biome[biome_pad_idx(px as usize, pz as usize)]
        } else {
            0
        }
    }
}

const FACE_MASK_WORDS: usize = SECTION_VOLUME / u64::BITS as usize;
type ExposedMasks = [[u64; FACE_MASK_WORDS]; FACES.len()];

#[inline]
fn mask_bit(i: usize) -> (usize, u64) {
    (i / u64::BITS as usize, 1u64 << (i % u64::BITS as usize))
}

#[inline]
fn mask_set(masks: &mut ExposedMasks, face: Face, cell: usize) {
    let (word, bit) = mask_bit(cell);
    masks[face_index(face)][word] |= bit;
}

#[inline]
fn mask_has(masks: &ExposedMasks, face: Face, cell: usize) -> bool {
    let (word, bit) = mask_bit(cell);
    masks[face_index(face)][word] & bit != 0
}

#[inline]
fn pad_cube_fast_candidate(block: Block) -> bool {
    block != Block::Water
        && block != Block::Cactus
        && block.render_shape() == RenderShape::Cube
        && block != Block::Chest
}

fn build_exposed_masks(pad: &SectionMeshPad<'_>) -> ExposedMasks {
    const CENTER_BITS: u32 = (1u32 << SECTION_SIZE) - 1;

    #[inline]
    fn row_idx(y: usize, z: usize) -> usize {
        y * SECTION_PAD + z
    }

    #[inline]
    fn set_face_row(masks: &mut ExposedMasks, face: Face, ly: usize, lz: usize, mut bits: u32) {
        while bits != 0 {
            let lx = bits.trailing_zeros() as usize;
            mask_set(masks, face, section_idx(lx, ly, lz));
            bits &= bits - 1;
        }
    }

    let mut masks = [[0u64; FACE_MASK_WORDS]; FACES.len()];
    let mut opaque_rows = [0u32; SECTION_PAD * SECTION_PAD];
    for py in 0..SECTION_PAD {
        for pz in 0..SECTION_PAD {
            let mut row = 0u32;
            for px in 0..SECTION_PAD {
                if pad.block_at_pad(px, py, pz).is_opaque() {
                    row |= 1u32 << px;
                }
            }
            opaque_rows[row_idx(py, pz)] = row;
        }
    }

    let mut candidate_rows = [0u32; SECTION_SIZE * SECTION_SIZE];
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let mut row = 0u32;
            for lx in 0..SECTION_SIZE {
                let block = pad.block_at_pad(lx + 1, ly + 1, lz + 1);
                if block == Block::Air || !pad_cube_fast_candidate(block) {
                    continue;
                }
                row |= 1u32 << lx;
            }
            candidate_rows[ly * SECTION_SIZE + lz] = row;
        }
    }

    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let cand = candidate_rows[ly * SECTION_SIZE + lz];
            if cand == 0 {
                continue;
            }
            let (py, pz) = (ly + 1, lz + 1);
            let x_row = opaque_rows[row_idx(py, pz)];
            set_face_row(
                &mut masks,
                Face::PosX,
                ly,
                lz,
                cand & !((x_row >> 2) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::NegX,
                ly,
                lz,
                cand & !(x_row & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::PosY,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py + 1, pz)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::NegY,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py - 1, pz)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::PosZ,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py, pz + 1)] >> 1) & CENTER_BITS),
            );
            set_face_row(
                &mut masks,
                Face::NegZ,
                ly,
                lz,
                cand & !((opaque_rows[row_idx(py, pz - 1)] >> 1) & CENTER_BITS),
            );
        }
    }
    masks
}

#[cfg(test)]
struct MeshContext {
    neighbour_water: fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: fn(i32, i32) -> bool,
}

#[cfg(test)]
impl MeshContext {
    fn standalone() -> Self {
        Self {
            neighbour_water: |_, _, _| 0,
            neighbour_chunk_loaded: |_, _| true,
        }
    }
}

#[cfg(test)]
pub fn build_mesh(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
) -> ChunkMesh {
    let ctx = MeshContext::standalone();
    build_mesh_with_context(
        chunk,
        neighbour_block,
        ctx.neighbour_water,
        neighbour_biome,
        neighbour_light,
        |_, _, _| 0,
        ctx.neighbour_chunk_loaded,
        MeshOptions::DETAILED,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn build_mesh_lods_with_loaded_neighbors(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
) -> ChunkMesh {
    let (ox, oz) = chunk.chunk_origin_world();
    let tints = tint::biome_window(ox, oz, &neighbour_biome);
    let mut mesh = chunk_geometry(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_chunk_loaded,
        &tints,
        MeshOptions::DETAILED,
    );
    if !chunk.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = chunk_geometry(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_chunk_loaded,
        &tints,
        MeshOptions::FAR_LEAVES,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}

#[cfg(test)]
pub fn build_mesh_with_options(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    options: MeshOptions,
) -> ChunkMesh {
    let ctx = MeshContext::standalone();
    build_mesh_with_context(
        chunk,
        neighbour_block,
        ctx.neighbour_water,
        neighbour_biome,
        neighbour_light,
        |_, _, _| 0,
        ctx.neighbour_chunk_loaded,
        options,
    )
}

/// Build the mesh for one cubic [`Section`]. All neighbour lookups are by WORLD
/// coordinate and route to the owning section (including this one), so the same
/// closure handles in-section and cross-section reads; out-of-world / unloaded
/// reads return air / open-sky as the closures define. Block-entity state (furnace
/// lit/facing, torch placement, model offset/facing) is read from `section`
/// directly. The renderer culls the resulting mesh by its [`SectionPos`].
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn build_section_mesh(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_stair_facing: impl Fn(i32, i32, i32) -> Facing,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    let tints = section.has_biome_tint_blocks().then(|| {
        let (ox, _, oz) = pos.origin_world();
        tint::biome_window(ox, oz, &neighbour_biome)
    });
    let mut mesh = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_stair_facing,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        tints.as_ref(),
        MeshOptions::DETAILED,
        None,
    );
    if !section.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_stair_facing,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
        None,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}

pub(crate) fn build_section_mesh_from_pad(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
) -> ChunkMesh {
    let (ox, oy, oz) = pos.origin_world();
    let nb_block = |wx, wy, wz| pad.block_world(ox, oy, oz, wx, wy, wz);
    let nb_stair_facing = |wx, wy, wz| pad.stair_world(ox, oy, oz, wx, wy, wz);
    let nb_water = |wx, wy, wz| pad.water_world(ox, oy, oz, wx, wy, wz);
    let nb_biome = |wx, wz| pad.biome_world(ox, oz, wx, wz);
    let nb_skylight = |wx, wy, wz| pad.skylight_world(ox, oy, oz, wx, wy, wz);
    let nb_blocklight = |wx, wy, wz| pad.blocklight_world(ox, oy, oz, wx, wy, wz);
    let nb_loaded = |wx, wy, wz| pad.loaded_world(ox, oy, oz, wx, wy, wz);
    let tints = section
        .has_biome_tint_blocks()
        .then(|| tint::biome_window(ox, oz, &nb_biome));
    let mut mesh = section_geometry(
        section,
        pos,
        &nb_block,
        &nb_stair_facing,
        &nb_water,
        &nb_skylight,
        &nb_blocklight,
        &nb_loaded,
        tints.as_ref(),
        MeshOptions::DETAILED,
        Some(&pad),
    );
    if !section.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = section_geometry(
        section,
        pos,
        &nb_block,
        &nb_stair_facing,
        &nb_water,
        &nb_skylight,
        &nb_blocklight,
        &nb_loaded,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
        None,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}

#[allow(clippy::too_many_arguments)]
fn section_geometry(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_stair_facing: impl Fn(i32, i32, i32) -> Facing,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
    tints: Option<&tint::BiomeTints>,
    options: MeshOptions,
    pad: Option<&SectionMeshPad<'_>>,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];
    let mut model: Vec<ModelVertex> = vec![];
    let mut model_idx: Vec<u32> = vec![];

    let (ox, oy, oz) = pos.origin_world();
    let tint_tile = |kind, ci| tints.map_or(tint::NO_TINT, |t| t.tile(kind, ci));
    let tint_grass = |ci| tints.map_or(tint::NO_TINT, |t| t.grass[ci]);
    let tint_water = |ci| tints.map_or(tint::NO_TINT, |t| t.water[ci]);

    // Every block read is by world coord through the routing closure (in-section
    // and cross-section alike); out-of-world / unloaded reads return air.
    let block_at =
        |wx: i32, wy: i32, wz: i32| -> Block { Block::from_id(neighbour_block(wx, wy, wz)) };
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 { neighbour_water(wx, wy, wz) };
    let fluid_at = |wx: i32, wy: i32, wz: i32| -> Option<f32> {
        if block_at(wx, wy, wz) != Block::Water {
            return None;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        Some(crate::world::water::fluid_height(
            water_at(wx, wy, wz),
            above,
        ))
    };
    let water_fills_cell = |wx: i32, wy: i32, wz: i32| -> bool {
        if block_at(wx, wy, wz) != Block::Water {
            return false;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        crate::world::water::fills_cell(water_at(wx, wy, wz), above)
    };

    // Reused per-thread greedy scratch: flat opaque cube faces are deferred here during the
    // cell scan, then merged into tiled quads after it. Taken out + put back so meshing
    // allocates nothing.
    let mut greedy = GREEDY.with(|g| {
        g.replace(GreedyScratch {
            faces: Vec::new(),
            merged: Vec::new(),
            gen: 0,
            slice_counts: [0; FACES.len() * SECTION_SIZE],
        })
    });
    let greedy_gen = greedy.begin();
    let exposed_masks = pad
        .filter(|_| options.leaf_mesh_mode == LeafMeshMode::Detailed)
        .map(build_exposed_masks);

    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let id = section.block_raw(lx, ly, lz);
                let block = Block::from_id(id);
                if block == Block::Air {
                    continue;
                }
                if block == Block::Chest {
                    continue;
                }
                // Resolve the render shape once per cell (each call indexes the block
                // table); the special-shape checks below and the cube fallthrough share it.
                let shape = block.render_shape();
                if shape == RenderShape::Door {
                    continue;
                }

                let wx = ox + lx as i32;
                let wy = oy + ly as i32;
                let wz = oz + lz as i32;
                let ci = lz * SECTION_SIZE + lx;

                if shape == RenderShape::Cross {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tint_tile(tile.world_tint(), ci), warm);
                    emit_cross(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        tile,
                        tint,
                        sky6,
                        block6,
                    );
                    continue;
                }

                if shape == RenderShape::Torch {
                    let [top_tile, _bottom, side_tile] = block.tiles();
                    // Sky channel = the cell's skylight; block channel = the torch's own
                    // emission (self-lit). `max(sky_term, block_term)` in the shader
                    // equals the old single-channel `max(cell_sky, emission)` fold at
                    // identity scale, and the emission channel never dims at night.
                    let cell_sky = neighbour_light(wx, wy, wz) as u32;
                    let sky6 = ((cell_sky * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let emit = block.light_emission() as u32;
                    let block6 = ((emit * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let placement = section.torch_placement(lx, ly, lz);
                    super::torch::emit_torch(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        placement,
                        side_tile,
                        top_tile,
                        [1.0, 1.0, 1.0],
                        sky6,
                        block6,
                    );
                    continue;
                }

                if let RenderShape::Model(kind) = shape {
                    let offset = section.model_offset(lx, ly, lz);
                    let facing = section.model_facing(lx, ly, lz);
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    emit_model_block(
                        &mut model,
                        &mut model_idx,
                        kind,
                        offset,
                        facing,
                        wx,
                        wy,
                        wz,
                        sky6,
                        block6,
                        warm,
                    );
                    continue;
                }

                if shape == RenderShape::Stair {
                    let [tile_top, tile_bot, tile_side] = block.tiles();
                    let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                    let facing = section.stair_facing(lx, ly, lz);
                    let mask = crate::stair::resolved_mask(IVec3::new(wx, wy, wz), facing, |p| {
                        crate::stair::is_stair(block_at(p.x, p.y, p.z))
                            .then(|| neighbour_stair_facing(p.x, p.y, p.z))
                    });
                    super::stair::emit_stair_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        mask,
                        [tile_top, tile_bot, tile_side],
                        &tint_for,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    continue;
                }

                let is_water = block == Block::Water;
                let [tile_top, tile_bot, tile_side] = block.tiles();
                let furnace_faces = (block == Block::Furnace).then(|| {
                    let front = if section.is_furnace_lit(lx, ly, lz) {
                        crate::atlas::engine().furnace_front_on
                    } else {
                        crate::atlas::engine().furnace_front
                    };
                    (facing_face(section.furnace_facing(lx, ly, lz)), front)
                });
                let base_x = wx as f32;
                let base_z = wz as f32;
                let base_y = wy as f32;

                let water_surface = is_water.then(|| {
                    let full = water_fills_cell(wx, wy, wz);
                    WaterSurface::new(wx, wy, wz, full, &block_at, &fluid_at)
                });

                if let (Some(pad), Some(exposed)) = (pad, exposed_masks.as_ref()) {
                    if pad_cube_fast_candidate(block) {
                        let cell = section_idx(lx, ly, lz);
                        for face in FACES {
                            if !mask_has(exposed, face, cell) {
                                continue;
                            }
                            let is_side =
                                matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                            let (base_tile, overlay_tile, tint) = if block == Block::Grass
                                && is_side
                            {
                                {
                                    let e = crate::atlas::engine();
                                    (e.dirt, Some(e.grass_side_overlay), tint_grass(ci))
                                }
                            } else {
                                let t = match face {
                                    Face::PosY => tile_top,
                                    Face::NegY => tile_bot,
                                    _ => match furnace_faces {
                                        Some((front_face, front_tile)) if face == front_face => {
                                            front_tile
                                        }
                                        Some(_) => crate::atlas::engine().furnace_side,
                                        None => tile_side,
                                    },
                                };
                                let tint = tint_tile(t.world_tint(), ci);
                                (t, None, tint)
                            };
                            let corners = quad_for(face, base_x, base_y, base_z);
                            let (dx, dy, dz) = face.dir();
                            let (fxp, fyp, fzp) = (
                                (lx as i32 + 1 + dx) as usize,
                                (ly as i32 + 1 + dy) as usize,
                                (lz as i32 + 1 + dz) as usize,
                            );
                            let fpi = mesh_pad_idx(fxp, fyp, fzp);
                            let f_l = pad.skylight[fpi] as u32;
                            let f_bl = pad.blocklight[fpi] as u32;
                            let (overlay, has_overlay) = match overlay_tile {
                                Some(o) => (o.index() as u32, true),
                                None => (0, false),
                            };
                            let (ao, light6, block6, warm) =
                                cube_face_lighting_pad(pad, face, fxp, fyp, fzp, f_l, f_bl, true);
                            let flat = ao[0] == ao[1]
                                && ao[1] == ao[2]
                                && ao[2] == ao[3]
                                && light6[0] == light6[1]
                                && light6[1] == light6[2]
                                && light6[2] == light6[3]
                                && block6[0] == block6[1]
                                && block6[1] == block6[2]
                                && block6[2] == block6[3]
                                && warm[0] == warm[1]
                                && warm[1] == warm[2]
                                && warm[2] == warm[3];
                            if overlay_tile.is_none() && block.is_opaque() && flat {
                                let final_tint = if warm[0] == 0.0 {
                                    tint
                                } else {
                                    warm_tint(tint, warm[0])
                                };
                                let fi = face_index(face);
                                greedy.faces[fi * SECTION_VOLUME + cell] = FlatFace {
                                    gen: greedy_gen,
                                    tile: base_tile.index() as u32,
                                    ao: ao[0],
                                    light6: light6[0],
                                    block6: block6[0],
                                    tint: final_tint,
                                };
                                let s = [lx, ly, lz][face_axes(face).0];
                                greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                            } else {
                                push_cube_face(
                                    &mut opaque,
                                    &mut opaque_idx,
                                    corners,
                                    base_tile,
                                    overlay,
                                    has_overlay,
                                    UV_MODE_NONE,
                                    tint,
                                    face,
                                    ao,
                                    light6,
                                    block6,
                                    warm,
                                );
                            }
                        }
                        continue;
                    }
                }

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nwx = wx + dx;
                    let nwy = wy + dy;
                    let nwz = wz + dz;
                    let nb = block_at(nwx, nwy, nwz);

                    let is_water_top = is_water && matches!(face, Face::PosY);
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                    let is_cactus_side = block == Block::Cactus && is_side;
                    if nb.is_opaque() && !is_water_top && !is_cactus_side {
                        continue;
                    }
                    if is_water && is_side && !neighbour_loaded(nwx, nwy, nwz) {
                        continue;
                    }
                    if options.leaf_mesh_mode == LeafMeshMode::Simplified
                        && block == Block::OakLeaves
                        && nb == Block::OakLeaves
                    {
                        continue;
                    }
                    let mut water_exposed_step = false;
                    if let Some(ws) = &water_surface {
                        if nb == Block::Water {
                            let nb_full = water_fills_cell(nwx, nwy, nwz);
                            match ws.side_against_water(is_side, nb_full) {
                                SideVsWater::ExposedStep => water_exposed_step = true,
                                SideVsWater::Cull => continue,
                            }
                        }
                    }

                    let (base_tile, overlay_tile, tint) = if let Some(ws) = &water_surface {
                        let t = match face {
                            Face::PosY => ws.top_tile(),
                            Face::NegY => crate::atlas::engine().water_still,
                            _ => crate::atlas::engine().water_flow,
                        };
                        (t, None, tint_water(ci))
                    } else if block == Block::Grass && is_side {
                        {
                            let e = crate::atlas::engine();
                            (e.dirt, Some(e.grass_side_overlay), tint_grass(ci))
                        }
                    } else {
                        let t = match face {
                            Face::PosY => tile_top,
                            Face::NegY => tile_bot,
                            _ => match furnace_faces {
                                Some((front_face, front_tile)) if face == front_face => front_tile,
                                Some(_) => crate::atlas::engine().furnace_side,
                                None => tile_side,
                            },
                        };
                        let tint = tint_tile(t.world_tint(), ci);
                        (t, None, tint)
                    };

                    let mut corners = if block == Block::Cactus {
                        cactus_quad(
                            face,
                            [base_x, base_y, base_z],
                            [base_x + 1.0, base_y + 1.0, base_z + 1.0],
                        )
                    } else {
                        quad_for(face, base_x, base_y, base_z)
                    };
                    if let Some(ws) = &water_surface {
                        ws.warp_quad(&mut corners, base_x, base_y, base_z, water_exposed_step);
                    }

                    let fx = nwx;
                    let fy = nwy;
                    let fz = nwz;
                    let f_l = neighbour_light(fx, fy, fz) as u32;
                    let f_bl = neighbour_blocklight(fx, fy, fz) as u32;

                    let water_ov: u32 = match &water_surface {
                        Some(ws) if matches!(face, Face::PosY) => ws.top_angle(),
                        _ => 0,
                    };
                    let (overlay, has_overlay) = match overlay_tile {
                        Some(o) => (o.index() as u32, true),
                        None => (water_ov, false),
                    };

                    let (ao, light6, block6, warm) = cube_face_lighting(
                        face,
                        fx,
                        fy,
                        fz,
                        f_l,
                        f_bl,
                        true,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    // Defer PLAIN opaque cube faces that are FLAT (all four corners share
                    // AO + light + warm) to the greedy merge — a run of them collapses into
                    // one tiled quad, pixel-identical. Water / grass-side (overlay) / leaves /
                    // cactus and any gradient (non-flat) face emit per-cell here, unchanged.
                    let flat = ao[0] == ao[1]
                        && ao[1] == ao[2]
                        && ao[2] == ao[3]
                        && light6[0] == light6[1]
                        && light6[1] == light6[2]
                        && light6[2] == light6[3]
                        && block6[0] == block6[1]
                        && block6[1] == block6[2]
                        && block6[2] == block6[3]
                        && warm[0] == warm[1]
                        && warm[1] == warm[2]
                        && warm[2] == warm[3];
                    if !is_water && overlay_tile.is_none() && block.is_opaque() && flat {
                        let final_tint = if warm[0] == 0.0 {
                            tint
                        } else {
                            warm_tint(tint, warm[0])
                        };
                        let fi = face_index(face);
                        greedy.faces[fi * SECTION_VOLUME + section_idx(lx, ly, lz)] = FlatFace {
                            gen: greedy_gen,
                            tile: base_tile.index() as u32,
                            ao: ao[0],
                            light6: light6[0],
                            block6: block6[0],
                            tint: final_tint,
                        };
                        // Slice index = the cell's coord along this face's normal axis.
                        let s = [lx, ly, lz][face_axes(face).0];
                        greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                    } else {
                        let (vbuf, ibuf) = if is_water {
                            (&mut transparent, &mut transparent_idx)
                        } else {
                            (&mut opaque, &mut opaque_idx)
                        };
                        let tris = push_cube_face(
                            vbuf,
                            ibuf,
                            corners,
                            base_tile,
                            overlay,
                            has_overlay,
                            UV_MODE_NONE,
                            tint,
                            face,
                            ao,
                            light6,
                            block6,
                            warm,
                        );
                        if is_water && matches!(face, Face::PosY) {
                            ibuf.extend_from_slice(&water::top_back_winding(tris));
                        }
                    }
                }
            }
        }
    }

    // Collapse the deferred flat faces into merged tiled quads, then return the scratch to
    // the thread-local for the next section.
    emit_greedy_quads(&mut greedy, &mut opaque, &mut opaque_idx, ox, oy, oz);
    GREEDY.with(|g| *g.borrow_mut() = greedy);

    ChunkMesh {
        opaque,
        opaque_idx,
        transparent,
        transparent_idx,
        far_opaque: vec![],
        far_opaque_idx: vec![],
        model,
        model_idx,
        mesh_dirty: true,
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_mesh_with_context(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
    options: MeshOptions,
) -> ChunkMesh {
    let (ox, oz) = chunk.chunk_origin_world();
    let tints = tint::biome_window(ox, oz, &neighbour_biome);
    chunk_geometry(
        chunk,
        neighbour_block,
        neighbour_water,
        neighbour_light,
        neighbour_blocklight,
        neighbour_chunk_loaded,
        &tints,
        options,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn chunk_geometry(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
    tints: &tint::BiomeTints,
    options: MeshOptions,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];
    // bbmodel-block geometry (explicit-UV, model atlas), drawn in the dedicated model
    // pass. Empty for the common chunk.
    let mut model: Vec<ModelVertex> = vec![];
    let mut model_idx: Vec<u32> = vec![];

    let (ox, oz) = chunk.chunk_origin_world();

    // The block at world coords, for AO/light neighbourhood sampling. Mirrors the
    // face-cull bounds logic: read this chunk directly when the column is
    // in-bounds, else defer to the neighbour lookup. Out-of-range Y and missing
    // neighbours read as air, so AO fades to fully-lit at the world's vertical
    // edges and at unloaded chunk borders. Callers pick `occludes_ao()` (AO, incl.
    // leaves) vs `is_opaque()` (which cells carry light) as needed.
    let block_at = |wx: i32, wy: i32, wz: i32| -> Block {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return Block::Air;
        }
        let lx = wx - ox;
        let lz = wz - oz;
        let id = if lx >= 0 && lx < CHUNK_SX as i32 && lz >= 0 && lz < CHUNK_SZ as i32 {
            chunk.block_raw(lx as usize, wy as usize, lz as usize)
        } else {
            neighbour_block(wx, wy, wz)
        };
        Block::from_id(id)
    };

    // Flowing-water metadata at a world voxel: this chunk in-bounds, else the
    // neighbour lookup (0 = source/none).
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return 0;
        }
        let lx = wx - ox;
        let lz = wz - oz;
        if lx >= 0 && lx < CHUNK_SX as i32 && lz >= 0 && lz < CHUNK_SZ as i32 {
            chunk.water_meta(lx as usize, wy as usize, lz as usize)
        } else {
            neighbour_water(wx, wy, wz)
        }
    };
    // Fluid surface height (0..1) of the water cell at a world voxel, or `None`
    // if it is not water. Water with water directly above fills to the top.
    let fluid_at = |wx: i32, wy: i32, wz: i32| -> Option<f32> {
        if block_at(wx, wy, wz) != Block::Water {
            return None;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        Some(crate::world::water::fluid_height(
            water_at(wx, wy, wz),
            above,
        ))
    };
    let water_fills_cell = |wx: i32, wy: i32, wz: i32| -> bool {
        if block_at(wx, wy, wz) != Block::Water {
            return false;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        crate::world::water::fills_cell(water_at(wx, wy, wz), above)
    };

    // Skip the all-air shell above the terrain. `heightmap[i]` is the highest
    // non-air Y in column i (set for every non-air block incl. water; rebuilt by
    // recompute_heightmap when block data arrives raw -- see worker.rs). Bounding
    // the outer loop by the chunk-wide max is byte-identical to looping 0..CHUNK_SY:
    // every skipped iteration (y > max_h) has an air centre voxel that would hit
    // the `Block::Air { continue }` guard below and emit zero bytes. We use the
    // chunk-wide max (NOT a per-column bound) so the y-major emission order -- and
    // thus the alpha-blended transparent buffer ordering -- is exactly preserved.
    let max_h = chunk.heightmap.iter().copied().max().unwrap_or(0) as usize;
    for y in 0..=max_h {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let id = chunk.block_raw(x, y, z);
                let block = Block::from_id(id);
                if block == Block::Air {
                    continue;
                }

                // Chests are not meshed into the chunk: their inset body + hinged lid
                // are drawn each frame as a dynamic model (see render::chest_model), so
                // the chunk emits nothing here. The block stays SOLID (collision /
                // raycast) but non-opaque, so neighbours keep their faces toward it and
                // there's no hole behind the inset model.
                if block == Block::Chest {
                    continue;
                }

                // Doors are not meshed into the chunk either: each is drawn every frame
                // as a dynamic hinged slab (see render::door_model) so it can swing. The
                // block stays SOLID (collision / raycast) but non-opaque, so neighbours
                // keep their faces toward the thin panel.
                if block.render_shape() == RenderShape::Door {
                    continue;
                }

                // Cross-model plants: two diagonal billboard quads in the opaque
                // (cutout) pass, then skip the cube face loop. They never cull or
                // get culled (non-opaque), carry no directional shade or AO, and
                // sample their own cell's skylight.
                if block.render_shape() == RenderShape::Cross {
                    let ci = z * CHUNK_SX + x;
                    let tile = block.tiles()[0];
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    // Fold skylight + torch block-light: a flower near a torch
                    // brightens and takes the same warm tint as the blocks around it.
                    let l = neighbour_light(wx, y as i32, wz) as u32;
                    let bl = neighbour_blocklight(wx, y as i32, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tints.tile(tile.world_tint(), ci), warm);
                    emit_cross(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        y as f32,
                        wz as f32,
                        tile,
                        tint,
                        sky6,
                        block6,
                    );
                    continue;
                }

                // Torch: a thin 3D pole (floor or wall-mounted), baked into the
                // opaque pass with its orientation read from this chunk's torch map.
                // Self-lit to at least its own emission so it stays visible/glowing
                // even where skylight is 0; the surrounding warm glow is the
                // block-light flood sampled by the cube faces below.
                if block.render_shape() == RenderShape::Torch {
                    let [top_tile, _bottom, side_tile] = block.tiles();
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    // Split channels: cell sky vs the torch's own emission (see the
                    // section-path torch emit for the identity argument).
                    let cell_sky = neighbour_light(wx, y as i32, wz) as u32;
                    let sky6 = ((cell_sky * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let emit = block.light_emission() as u32;
                    let block6 = ((emit * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let placement = chunk.torch_placement(x, y, z);
                    super::torch::emit_torch(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        y as f32,
                        wz as f32,
                        placement,
                        side_tile,
                        top_tile,
                        [1.0, 1.0, 1.0],
                        sky6,
                        block6,
                    );
                    continue;
                }

                // bbmodel blocks: NOT packed into the legacy mesh. Their geometry rides
                // the explicit-UV `model` stream (own texture/atlas), but they're still
                // chunk-meshed here — baked once per remesh and lit at mesh time exactly
                // like any block. A multi-block cell renders only ITS footprint cubes
                // (the split by `model_offset`); a missing-neighbour face isn't culled
                // (the block is non-opaque), so it reads as a placed object, not a cube.
                if let RenderShape::Model(kind) = block.render_shape() {
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let offset = chunk.model_offset(x, y, z);
                    let facing = chunk.model_facing(x, y, z);
                    let l = neighbour_light(wx, y as i32, wz) as u32;
                    let bl = neighbour_blocklight(wx, y as i32, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    emit_model_block(
                        &mut model,
                        &mut model_idx,
                        kind,
                        offset,
                        facing,
                        wx,
                        y as i32,
                        wz,
                        sky6,
                        block6,
                        warm,
                    );
                    continue;
                }

                if block.render_shape() == RenderShape::Stair {
                    let [tile_top, tile_bot, tile_side] = block.tiles();
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let ci = z * CHUNK_SX + x;
                    let tint_for = |tile: Tile| tints.tile(tile.world_tint(), ci);
                    let mask = crate::stair::mask(crate::block_model::DEFAULT_MODEL_FACING);
                    super::stair::emit_stair_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        y as i32,
                        wz,
                        mask,
                        [tile_top, tile_bot, tile_side],
                        &tint_for,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    continue;
                }

                // Only water is alpha-blended; leaves render in the OPAQUE pass
                // (crisp/cutout, no see-through ghosting) per the "fully opaque" rule.
                let is_water = block == Block::Water;

                // Choose tile for each face.
                let [tile_top, tile_bot, tile_side] = block.tiles();
                // A furnace shows its front (lit/unlit) only on the face it was
                // placed facing; the other three horizontal faces use furnace_side.
                // Read from this chunk's own furnace map (in-chunk only — facing
                // never affects cross-border culling).
                let furnace_faces = (block == Block::Furnace).then(|| {
                    let front = if chunk.is_furnace_lit(x, y, z) {
                        crate::atlas::engine().furnace_front_on
                    } else {
                        crate::atlas::engine().furnace_front
                    };
                    (facing_face(chunk.furnace_facing(x, y, z)), front)
                });
                let ci = z * CHUNK_SX + x;
                let base_x = x as f32 + ox as f32;
                let base_z = z as f32 + oz as f32;

                // Water surface geometry (corner heights, flow tile/heading, full-
                // height flag), computed once per cell — see `mesh::water`. `None`
                // for non-water blocks.
                let water_surface = is_water.then(|| {
                    let full = water_fills_cell(ox + x as i32, y as i32, oz + z as i32);
                    WaterSurface::new(
                        ox + x as i32,
                        y as i32,
                        oz + z as i32,
                        full,
                        &block_at,
                        &fluid_at,
                    )
                });

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    let nz = z as i32 + dz;

                    // Neighbour block to test cull.
                    let mut unloaded_horizontal_neighbor = false;
                    let nb_id =
                        if nx < 0 || nx >= CHUNK_SX as i32 || nz < 0 || nz >= CHUNK_SZ as i32 {
                            // Out of horizontal chunk bounds -> ask neighbour fn.
                            let wx = ox + nx;
                            let wz = oz + nz;
                            unloaded_horizontal_neighbor =
                                !neighbour_chunk_loaded(wx >> 4, wz >> 4);
                            if ny < 0 || ny >= CHUNK_SY as i32 {
                                0 // air
                            } else {
                                neighbour_block(wx, ny, wz)
                            }
                        } else if ny < 0 || ny >= CHUNK_SY as i32 {
                            0
                        } else {
                            chunk.block_raw(nx as usize, ny as usize, nz as usize)
                        };
                    let nb = Block::from_id(nb_id);

                    // Cull rule: a face is hidden only if the neighbour is a full
                    // opaque cube (`is_opaque()` -- stone/dirt/grass/sand/snow/log).
                    // Leaves are NOT opaque-for-culling (they're a cutout), so
                    // leaf<->leaf faces are intentionally NOT culled -- every leaf
                    // cube draws all its faces, giving a dense canopy you can't see
                    // through to the sky. Water additionally culls against itself.
                    //
                    // Exception: water's TOP face is kept even under an opaque
                    // block. The surface sits recessed below the block, so culling
                    // it punches a hole (the floor shows through where a block caps
                    // the water). Water's bottom/side faces against opaque still cull.
                    let is_water_top = is_water && matches!(face, Face::PosY);
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                    // A cactus's four sides are inset 1px (see `cactus_quad`), so they stay
                    // visible in the gap even when a solid block sits flush against the cell
                    // — never cull them. Its flush top/bottom DO cull normally, so the
                    // bottom hides against the (opaque) block the cactus rests on.
                    let is_cactus_side = block == Block::Cactus && is_side;
                    if nb.is_opaque() && !is_water_top && !is_cactus_side {
                        continue;
                    }
                    if is_water && is_side && unloaded_horizontal_neighbor {
                        continue;
                    }
                    if options.leaf_mesh_mode == LeafMeshMode::Simplified
                        && block == Block::OakLeaves
                        && nb == Block::OakLeaves
                    {
                        continue;
                    }
                    // Set on the one water-water side face we DON'T cull: a
                    // full-height cell's exposed step over a shorter neighbour.
                    // Its bottom is trimmed to the neighbour's surface (below) so
                    // the submerged part — water behind water — isn't drawn twice.
                    // Otherwise faces between two water cells cull (the surfaces meet).
                    let mut water_exposed_step = false;
                    if let Some(ws) = &water_surface {
                        if nb == Block::Water {
                            let nb_full = water_fills_cell(ox + nx, y as i32, oz + nz);
                            match ws.side_against_water(is_side, nb_full) {
                                SideVsWater::ExposedStep => water_exposed_step = true,
                                SideVsWater::Cull => continue,
                            }
                        }
                    }

                    // Material for this face: base tile + optional biome-tinted
                    // overlay + tint + texture rotation. Water tops use the still
                    // or flow tile (rotated toward the flow); water sides always
                    // use the downward-flowing tile. Grass block SIDES render as
                    // dirt + a grayscale grass overlay tinted by the same biome
                    // grass colour as the top. Everything else is the face's own
                    // tile, tinted only for grass-top/foliage/water.
                    let (base_tile, overlay_tile, tint) = if let Some(ws) = &water_surface {
                        let t = match face {
                            Face::PosY => ws.top_tile(),
                            Face::NegY => crate::atlas::engine().water_still,
                            _ => crate::atlas::engine().water_flow,
                        };
                        (t, None, tints.water[ci])
                    } else if block == Block::Grass && is_side {
                        let e = crate::atlas::engine();
                        (e.dirt, Some(e.grass_side_overlay), tints.grass[ci])
                    } else {
                        let t = match face {
                            Face::PosY => tile_top,
                            Face::NegY => tile_bot,
                            _ => match furnace_faces {
                                Some((front_face, front_tile)) if face == front_face => front_tile,
                                Some(_) => crate::atlas::engine().furnace_side,
                                None => tile_side,
                            },
                        };
                        let tint = tints.tile(t.world_tint(), ci);
                        (t, None, tint)
                    };

                    // Build quad vertices in CCW order when viewed from outside.
                    // Positions are in world space (baked chunk origin) so each
                    // chunk renders at its actual world coordinates.
                    let base_y = y as f32;
                    // The cactus recesses its four side faces 1/16 so its spines show; every
                    // other cube fills the cell. Top/bottom stay full (a flush stack of
                    // segments; the cap overhangs the trunk).
                    let mut corners = if block == Block::Cactus {
                        cactus_quad(
                            face,
                            [base_x, base_y, base_z],
                            [base_x + 1.0, base_y + 1.0, base_z + 1.0],
                        )
                    } else {
                        quad_for(face, base_x, base_y, base_z)
                    };

                    // Water vertices are warped onto the cell's surface (top edge to
                    // the per-corner height; exposed-step faces also trim the bottom).
                    if let Some(ws) = &water_surface {
                        ws.warp_quad(&mut corners, base_x, base_y, base_z, water_exposed_step);
                    }

                    // The front voxel F = block + normal, and its pre-sampled light:
                    // the shared seed for every corner's AO + smooth-skylight sample.
                    let fx = ox + x as i32 + dx;
                    let fy = y as i32 + dy;
                    let fz = oz + z as i32 + dz;
                    let f_l = neighbour_light(fx, fy, fz) as u32;
                    let f_bl = neighbour_blocklight(fx, fy, fz) as u32;

                    // Resolve this face's 12..20 overlay payload + has-overlay flag.
                    // A grass SIDE carries the tinted GrassSideOverlay; a flowing
                    // water TOP (no grass overlay) reuses those 8 bits to carry the
                    // quantized flow heading with the flag CLEAR, so the fragment
                    // shader composites no overlay (water side faces derive their
                    // texture V from the vertex height in the shader, so they need
                    // no per-face data here). `pack_vertex` owns the bit positions.
                    let water_ov: u32 = match &water_surface {
                        Some(ws) if matches!(face, Face::PosY) => ws.top_angle(),
                        _ => 0,
                    };
                    let (overlay, has_overlay) = match overlay_tile {
                        Some(o) => (o.index() as u32, true),
                        None => (water_ov, false),
                    };

                    let (vbuf, ibuf) = if is_water {
                        (&mut transparent, &mut transparent_idx)
                    } else {
                        (&mut opaque, &mut opaque_idx)
                    };

                    let tris = emit_cube_face(
                        vbuf,
                        ibuf,
                        corners,
                        base_tile,
                        overlay,
                        has_overlay,
                        UV_MODE_NONE,
                        tint,
                        face,
                        fx,
                        fy,
                        fz,
                        f_l,
                        f_bl,
                        true,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    // A water surface (top face) also emits its reverse winding so it
                    // stays visible from underneath in the back-face-culled
                    // transparent pass; side/bottom faces stay single-sided.
                    if is_water && matches!(face, Face::PosY) {
                        ibuf.extend_from_slice(&water::top_back_winding(tris));
                    }
                }
            }
        }
    }

    ChunkMesh {
        opaque,
        opaque_idx,
        transparent,
        transparent_idx,
        far_opaque: vec![],
        far_opaque_idx: vec![],
        model,
        model_idx,
        mesh_dirty: true,
    }
}

/// Mesh-time brightness for a bbmodel-block face from the cell's combined 6-bit light.
/// Mirrors `block.wgsl`'s skylight curve (the block pipeline applies this in the shader;
/// the model pass shader just multiplies, so we bake the curve in here) — keep the
/// constants in sync.
#[inline]
/// Stream one bbmodel-block cell's geometry into the `model` buffers: copy the cell's
/// startup-baked template (positions already taken through the cube rotation + placement
/// facing) translated to the world base, carrying the cell's (sky, block) light
/// separately so the world-model shader applies the day/night scale at draw time,
/// plus the warm block-light tint. No matrices / quaternions / face-bias work
/// happens per remesh — it's all resolved once in [`block_model::ModelInstance`],
/// so meshing a placed model is a translate + scale + copy.
#[allow(clippy::too_many_arguments)]
fn emit_model_block(
    verts: &mut Vec<ModelVertex>,
    indices: &mut Vec<u32>,
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
    wx: i32,
    wy: i32,
    wz: i32,
    sky6: u32,
    block6: u32,
    warm: f32,
) {
    let inst = block_model::instance(kind);
    let Some(tmpl) = inst.cell_template(offset, facing) else {
        return;
    };
    // The chunk stores the authored cell offset + placed facing; together those resolve the
    // rotated footprint base. The template's vertices are baked relative to that base, so
    // placing the cell is one translate per vertex.
    let base = block_model::base_from_cell(IVec3::new(wx, wy, wz), kind, offset, facing);
    let basef = Vec3::new(base.x as f32, base.y as f32, base.z as f32);
    let light = [
        (sky6 as f32 / 63.0).clamp(0.0, 1.0),
        (block6 as f32 / 63.0).clamp(0.0, 1.0),
    ];
    let tint = warm_tint([1.0, 1.0, 1.0], warm);
    let start = verts.len() as u32;
    verts.extend(tmpl.verts.iter().map(|v| ModelVertex {
        pos: (basef + v.pos).to_array(),
        uv: v.uv,
        shade: v.shade,
        tint,
        light,
    }));
    indices.extend(tmpl.indices.iter().map(|&i| start + i));
}

/// Fold a cell's (or neighbourhood-summed) skylight + block-light into TWO packed
/// 6-bit brightness channels `(sky6, block6)` plus a 0..1 warm amount.
/// `sum_sky`/`sum_block` are x2-scale sums over `denom = cnt * SKY_FULL` cells
/// (`cnt = 1`, `denom = SKY_FULL` for a single cell). The channels stay SEPARATE
/// in the vertex (`packed` bits 23..29 = sky, `packed2` bits 0..6 = block) so the
/// shader can dim the sky term without dimming torch light; the shader recombines
/// with `max(sky_term, block_term)`. Because the per-channel quantizer is monotone
/// non-decreasing, `max(sky6, block6) == quantize(max(sum_sky, sum_block))` — the
/// value the single channel used to hold — so at identity scale the final light is
/// bit-identical to the pre-split output. Warm comes from the shared
/// [`warm_amount`](crate::torch::warm_amount) so static blocks and dynamic
/// geometry warm identically.
#[inline]
fn fold_light(sum_sky: u32, sum_block: u32, denom: u32) -> (u32, u32, f32) {
    let sky6 = ((sum_sky * 63 + denom / 2) / denom).min(63);
    let block6 = ((sum_block * 63 + denom / 2) / denom).min(63);
    // `warm_amount` is `block01 * (1 - sky01) * strength`, so no block-light means
    // exactly 0.0 warmth — skip its two f32 divisions in the torch-free common case
    // (nearly all daylit terrain). Byte-identical: the multiply by a zero block term
    // yields +0.0, the same bits this returns.
    let warm = if sum_block == 0 {
        0.0
    } else {
        warm_amount(
            sum_sky as f32 / denom as f32,
            sum_block as f32 / denom as f32,
        )
    };
    (sky6, block6, warm)
}

/// Like [`fold_light`] but for the per-corner smooth-light mean over `cnt` cells
/// (`1..=4`). The divisor `cnt * SKY_FULL` is one of four constants, so matching on
/// `cnt` lets the compiler lower each arm's integer division to a multiply-shift —
/// removing the last per-corner division from the emit hot loop. Byte-identical to
/// `fold_light(sum_sky, sum_block, cnt * SKY_FULL)`.
#[inline]
fn fold_light_smooth(sum_sky: u32, sum_block: u32, cnt: u32) -> (u32, u32, f32) {
    #[inline(always)]
    fn quant(sum: u32, cnt: u32) -> u32 {
        let v = sum * 63;
        match cnt {
            1 => (v + 15) / 30,
            2 => (v + 30) / 60,
            3 => (v + 45) / 90,
            _ => (v + 60) / 120,
        }
        .min(63)
    }
    let sky6 = quant(sum_sky, cnt);
    // The torch-free common case (nearly all terrain) skips the block-channel
    // divide entirely: a zero sum quantizes to exactly 0.
    let block6 = if sum_block == 0 {
        0
    } else {
        quant(sum_block, cnt)
    };
    let warm = if sum_block == 0 {
        0.0
    } else {
        let denom = cnt * SKY_FULL as u32;
        warm_amount(
            sum_sky as f32 / denom as f32,
            sum_block as f32 / denom as f32,
        )
    };
    (sky6, block6, warm)
}

/// Emit one resolved cube face: gather the shared 3×3 tangent-plane ring around the
/// front voxel F once, derive each corner's AO + smooth light from it, push the four
/// packed vertices, and append the (AO-symmetric, possibly flipped) triangulation.
/// Returns the two triangles' six indices so the caller can add water's reverse winding
/// before closing the section. The `corners` are already in world space (and
/// water-warped); `face` drives shade + the AO neighbourhood; `(fx,fy,fz)`/`f_l`/`f_bl`
/// are the front voxel and its pre-sampled light.
///
/// Only the legacy chunk mesher composes lighting + push this way; the section
/// path calls `cube_face_lighting` / `push_cube_face` separately.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn emit_cube_face<B, L, K>(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    base_tile: Tile,
    overlay: u32,
    has_overlay: bool,
    uv_mode: u32,
    tint: [f32; 3],
    face: Face,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) -> [u32; 6]
where
    B: Fn(i32, i32, i32) -> Block,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
    let (ao, light6, block6, warm) = cube_face_lighting(
        face,
        fx,
        fy,
        fz,
        f_l,
        f_bl,
        smooth_light,
        block_at,
        neighbour_light,
        neighbour_blocklight,
    );
    push_cube_face(
        vbuf,
        ibuf,
        corners,
        base_tile,
        overlay,
        has_overlay,
        uv_mode,
        tint,
        face,
        ao,
        light6,
        block6,
        warm,
    )
}

/// One cube face's per-corner AO + smooth light (skylight/block-light + warm amount),
/// gathered from the shared 3×3 tangent-plane ring around the front voxel F ONCE. The
/// four corners share these eight ring cells (each edge cell feeds two corners, each
/// diagonal one), so a single gather replaces per-corner re-reads. `occ` = AO occluders
/// (opaque cubes AND leaves, for canopy self-occlusion); `opq` = full-opaque, which carry
/// no light and so are excluded from the smooth-light mean (leaves differ between the two,
/// hence both bits). The centre cell (a=b=0) is F itself and is never sampled, so skipped.
///
/// Split from the vertex push so the greedy mesher can test a face for flatness (all four
/// corners equal — the merge condition) before deciding to emit it per-cell or merge it.
#[allow(clippy::too_many_arguments)]
pub(super) fn cube_face_lighting<B, L, K>(
    face: Face,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) -> ([u32; 4], [u32; 4], [u32; 4], [f32; 4])
where
    B: Fn(i32, i32, i32) -> Block,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();

    let mut occ = [[false; 3]; 3];
    let mut opq = [[false; 3]; 3];
    let mut sky = [[0u32; 3]; 3];
    let mut blk = [[0u32; 3]; 3];
    for a in -1i32..=1 {
        for b in -1i32..=1 {
            if a == 0 && b == 0 {
                continue;
            }
            let (cx, cy, cz) = (
                fx + a * ux + b * vx,
                fy + a * uy + b * vy,
                fz + a * uz + b * vz,
            );
            let cell = block_at(cx, cy, cz);
            let (ia, ib) = ((a + 1) as usize, (b + 1) as usize);
            occ[ia][ib] = cell.occludes_ao();
            if smooth_light {
                opq[ia][ib] = cell.is_opaque();
                if !opq[ia][ib] {
                    sky[ia][ib] = neighbour_light(cx, cy, cz) as u32;
                    blk[ia][ib] = neighbour_blocklight(cx, cy, cz) as u32;
                }
            }
        }
    }

    // Per corner, resolve AO + light from the gathered ring: its two edge cells
    // (`[iu][1]` along u, `[1][iv]` along v) and its diagonal (`[iu][iv]`).
    let signs = face.ao_signs();
    let mut ao = [3u32; 4];
    let mut light6 = [0u32; 4];
    let mut block6 = [0u32; 4];
    let mut warm = [0f32; 4];
    let flat = fold_light(f_l, f_bl, SKY_FULL as u32);
    for corner in 0..4 {
        let (su, sv) = signs[corner];
        let (iu, iv) = ((su + 1) as usize, (sv + 1) as usize);
        ao[corner] = vertex_ao(occ[iu][1], occ[1][iv], occ[iu][iv]);
        if !smooth_light {
            (light6[corner], block6[corner], warm[corner]) = flat;
            continue;
        }
        let mut sum = f_l;
        let mut sum_block = f_bl;
        let mut cnt = 1u32;
        if !opq[iu][1] {
            sum += sky[iu][1];
            sum_block += blk[iu][1];
            cnt += 1;
        }
        if !opq[1][iv] {
            sum += sky[1][iv];
            sum_block += blk[1][iv];
            cnt += 1;
        }
        if !opq[iu][iv] {
            sum += sky[iu][iv];
            sum_block += blk[iu][iv];
            cnt += 1;
        }
        (light6[corner], block6[corner], warm[corner]) = fold_light_smooth(sum, sum_block, cnt);
    }
    (ao, light6, block6, warm)
}

fn cube_face_lighting_pad(
    pad: &SectionMeshPad<'_>,
    face: Face,
    fx: usize,
    fy: usize,
    fz: usize,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
) -> ([u32; 4], [u32; 4], [u32; 4], [f32; 4]) {
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();

    let mut occ = [[false; 3]; 3];
    let mut opq = [[false; 3]; 3];
    let mut sky = [[0u32; 3]; 3];
    let mut blk = [[0u32; 3]; 3];
    for a in -1i32..=1 {
        for b in -1i32..=1 {
            if a == 0 && b == 0 {
                continue;
            }
            let (cx, cy, cz) = (
                (fx as i32 + a * ux + b * vx) as usize,
                (fy as i32 + a * uy + b * vy) as usize,
                (fz as i32 + a * uz + b * vz) as usize,
            );
            let cell = pad.block_at_pad(cx, cy, cz);
            let i = mesh_pad_idx(cx, cy, cz);
            let (ia, ib) = ((a + 1) as usize, (b + 1) as usize);
            occ[ia][ib] = cell.occludes_ao();
            if smooth_light {
                opq[ia][ib] = cell.is_opaque();
                if !opq[ia][ib] {
                    sky[ia][ib] = pad.skylight[i] as u32;
                    blk[ia][ib] = pad.blocklight[i] as u32;
                }
            }
        }
    }

    let signs = face.ao_signs();
    let mut ao = [3u32; 4];
    let mut light6 = [0u32; 4];
    let mut block6 = [0u32; 4];
    let mut warm = [0f32; 4];
    let flat = fold_light(f_l, f_bl, SKY_FULL as u32);
    for corner in 0..4 {
        let (su, sv) = signs[corner];
        let (iu, iv) = ((su + 1) as usize, (sv + 1) as usize);
        ao[corner] = vertex_ao(occ[iu][1], occ[1][iv], occ[iu][iv]);
        if !smooth_light {
            (light6[corner], block6[corner], warm[corner]) = flat;
            continue;
        }
        let mut sum = f_l;
        let mut sum_block = f_bl;
        let mut cnt = 1u32;
        if !opq[iu][1] {
            sum += sky[iu][1];
            sum_block += blk[iu][1];
            cnt += 1;
        }
        if !opq[1][iv] {
            sum += sky[1][iv];
            sum_block += blk[1][iv];
            cnt += 1;
        }
        if !opq[iu][iv] {
            sum += sky[iu][iv];
            sum_block += blk[iu][iv];
            cnt += 1;
        }
        (light6[corner], block6[corner], warm[corner]) = fold_light_smooth(sum, sum_block, cnt);
    }
    (ao, light6, block6, warm)
}

/// Push one resolved cube face's four packed vertices (given precomputed per-corner
/// lighting) + its (AO-symmetric, possibly flipped) triangulation. Returns the six indices
/// so the caller can add water's reverse winding.
#[allow(clippy::too_many_arguments)]
fn push_cube_face(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    base_tile: Tile,
    overlay: u32,
    has_overlay: bool,
    uv_mode: u32,
    tint: [f32; 3],
    face: Face,
    ao: [u32; 4],
    light6: [u32; 4],
    block6: [u32; 4],
    warm: [f32; 4],
) -> [u32; 6] {
    let shade_idx = face.shade_idx();
    let start = vbuf.len() as u32;
    for (corner, p) in corners.into_iter().enumerate() {
        vbuf.push(Vertex {
            pos: p,
            // Warm the face tint per corner by however much torch light reaches it,
            // so the glow fades smoothly across the surface (0 warm = unchanged, so
            // skip the multiply entirely — the torch-free common case).
            tint: if warm[corner] == 0.0 {
                tint
            } else {
                warm_tint(tint, warm[corner])
            },
            packed: pack_vertex(
                base_tile.index() as u32,
                corner as u32,
                shade_idx,
                overlay,
                has_overlay,
                ao[corner],
                light6[corner],
            ) | (uv_mode << UV_MODE_SHIFT),
            packed2: pack_vertex2(block6[corner]),
        });
    }
    // Flip the triangulation so the split runs along the darker diagonal -- keeps
    // the AO gradient symmetric (no bright bleed).
    let tris: [u32; 6] = if should_flip(ao) {
        [start, start + 1, start + 3, start + 1, start + 2, start + 3]
    } else {
        [start, start + 1, start + 2, start, start + 2, start + 3]
    };
    ibuf.extend_from_slice(&tris);
    tris
}

/// A flat (uniform-across-corners) opaque cube face, recorded per (direction, cell) so a
/// run of identical adjacent faces can collapse into ONE tiled quad (greedy meshing). Only
/// faces whose four corners share the same AO + light + tint + tile qualify — then the merged
/// quad, drawn flat with its layer tiled W×H (REPEAT sampler), is pixel-identical to the
/// per-cell faces it replaces. `gen` matches the current build's generation for a live face
/// (a generation counter avoids re-zeroing the whole 6×4096 scratch every section — the fixed
/// cost that otherwise ~doubled meshing throughput; a stale entry from a prior build has an
/// old `gen` and reads as absent).
#[derive(Copy, Clone, PartialEq)]
struct FlatFace {
    gen: u32,
    tile: u32,
    ao: u32,
    light6: u32,
    /// Second light channel (block light) — merges require BOTH channels equal, so
    /// a merged quad's `packed2` word is exact for every cell it replaces.
    block6: u32,
    tint: [f32; 3],
}

const FLAT_ABSENT: FlatFace = FlatFace {
    gen: 0,
    tile: 0,
    ao: 0,
    light6: 0,
    block6: 0,
    tint: [0.0; 3],
};

/// Reused per-thread greedy-merge scratch: a `FlatFace` per (face direction 0..6, cell), a
/// per-slice merged-flag grid, the current build generation, and a deferred-face count per
/// direction (to skip merging directions that received none). Thread-local + reused so meshing
/// a section allocates nothing AND clears nothing (the `gen` bump retires the prior build).
struct GreedyScratch {
    faces: Vec<FlatFace>,
    merged: Vec<bool>,
    gen: u32,
    /// Deferred-face count per (direction, slice), so the merge pass scans only the few slices
    /// that actually received flat faces instead of all 6×16 (empty slices dominate — flat
    /// faces cluster in the surface/floor layers).
    slice_counts: [u32; FACES.len() * SECTION_SIZE],
}

impl GreedyScratch {
    /// Retire the previous build and return this build's generation. No `faces` reset: a bumped
    /// `gen` makes every prior entry read as absent. Only allocates on first use per thread, and
    /// only re-zeroes on the (≈4-billion-build) `gen` wrap so a stale entry can't alias.
    fn begin(&mut self) -> u32 {
        if self.faces.len() != FACES.len() * SECTION_VOLUME {
            self.faces = vec![FLAT_ABSENT; FACES.len() * SECTION_VOLUME];
        }
        if self.merged.len() != SECTION_SIZE * SECTION_SIZE {
            self.merged = vec![false; SECTION_SIZE * SECTION_SIZE];
        }
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.gen = 1;
            self.faces.fill(FLAT_ABSENT);
        }
        self.slice_counts = [0; FACES.len() * SECTION_SIZE];
        self.gen
    }
}

thread_local! {
    static GREEDY: RefCell<GreedyScratch> = const {
        RefCell::new(GreedyScratch {
            faces: Vec::new(),
            merged: Vec::new(),
            gen: 0,
            slice_counts: [0; FACES.len() * SECTION_SIZE],
        })
    };
}

/// A cube face's `(normal, U, V)` local axes (0=X, 1=Y, 2=Z), derived from `Face::quad_box`
/// so the greedy slice's `(u,v)` grid and a merged quad's tiled UV (W tiles along U, H along
/// V) align with `corner_local`: normal-X → U=Z,V=Y; normal-Y → U=X,V=Z; normal-Z → U=X,V=Y.
#[inline]
fn face_axes(face: Face) -> (usize, usize, usize) {
    match face {
        Face::PosX | Face::NegX => (0, 2, 1),
        Face::PosY | Face::NegY => (1, 0, 2),
        Face::PosZ | Face::NegZ => (2, 0, 1),
    }
}

/// Index of a face in [`FACES`] — the per-direction plane in [`GreedyScratch::faces`]. Must
/// match `FACES.into_iter().enumerate()` in [`emit_greedy_quads`].
#[inline]
fn face_index(face: Face) -> usize {
    match face {
        Face::PosX => 0,
        Face::NegX => 1,
        Face::PosY => 2,
        Face::NegY => 3,
        Face::PosZ => 4,
        Face::NegZ => 5,
    }
}

/// Greedy-merge every deferred flat face (in `scratch.faces`) into the fewest tiled quads and
/// push them to the opaque buffers. For each direction and each 16-cell slice, it 2D-merges
/// maximal rectangles of identical `FlatFace`s (extend width along U, then height along V),
/// emitting one quad per rectangle with `(W-1, H-1)` packed so the shader tiles its layer.
fn emit_greedy_quads(
    scratch: &mut GreedyScratch,
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    ox: i32,
    oy: i32,
    oz: i32,
) {
    let cur = scratch.gen;
    let slice_counts = scratch.slice_counts;
    let key_at = |faces: &[FlatFace],
                  fi: usize,
                  n: usize,
                  s: usize,
                  ua: usize,
                  u: usize,
                  va: usize,
                  v: usize|
     -> FlatFace {
        let mut l = [0usize; 3];
        l[n] = s;
        l[ua] = u;
        l[va] = v;
        faces[fi * SECTION_VOLUME + section_idx(l[0], l[1], l[2])]
    };
    for (fi, face) in FACES.into_iter().enumerate() {
        let (n, ua, va) = face_axes(face);
        for s in 0..SECTION_SIZE {
            if slice_counts[fi * SECTION_SIZE + s] == 0 {
                continue; // no deferred faces in this slice — skip its 16×16 scan + fill.
            }
            scratch.merged.fill(false);
            for v in 0..SECTION_SIZE {
                for u in 0..SECTION_SIZE {
                    if scratch.merged[v * SECTION_SIZE + u] {
                        continue;
                    }
                    let key = key_at(&scratch.faces, fi, n, s, ua, u, va, v);
                    if key.gen != cur {
                        continue; // stale (prior build) or never written = absent.
                    }
                    // Extend the run along U while cells match and are unmerged.
                    let mut w = 1;
                    while u + w < SECTION_SIZE
                        && !scratch.merged[v * SECTION_SIZE + u + w]
                        && key_at(&scratch.faces, fi, n, s, ua, u + w, va, v) == key
                    {
                        w += 1;
                    }
                    // Extend along V while the whole W-wide row matches and is unmerged.
                    let mut h = 1;
                    'grow: while v + h < SECTION_SIZE {
                        for k in 0..w {
                            if scratch.merged[(v + h) * SECTION_SIZE + u + k]
                                || key_at(&scratch.faces, fi, n, s, ua, u + k, va, v + h) != key
                            {
                                break 'grow;
                            }
                        }
                        h += 1;
                    }
                    for dv in 0..h {
                        for du in 0..w {
                            scratch.merged[(v + dv) * SECTION_SIZE + u + du] = true;
                        }
                    }
                    let mut lmin = [0i32; 3];
                    let mut lmax = [0i32; 3];
                    lmin[n] = s as i32;
                    lmax[n] = s as i32 + 1;
                    lmin[ua] = u as i32;
                    lmax[ua] = (u + w) as i32;
                    lmin[va] = v as i32;
                    lmax[va] = (v + h) as i32;
                    let min = [
                        (ox + lmin[0]) as f32,
                        (oy + lmin[1]) as f32,
                        (oz + lmin[2]) as f32,
                    ];
                    let max = [
                        (ox + lmax[0]) as f32,
                        (oy + lmax[1]) as f32,
                        (oz + lmax[2]) as f32,
                    ];
                    push_greedy_quad(opaque, opaque_idx, face, min, max, key, w as u32, h as u32);
                }
            }
        }
    }
}

/// Push one greedy-merged quad: four flat vertices over the world box `[min,max]` with the
/// merge extents `(w,h)` packed into the overlay-tile bits (`(W-1) | (H-1)<<4`), which the
/// block shader reads to tile the layer. Uniform AO ⇒ no diagonal flip (default winding).
fn push_greedy_quad(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    face: Face,
    min: [f32; 3],
    max: [f32; 3],
    key: FlatFace,
    w: u32,
    h: u32,
) {
    let (min, max) = overlap_greedy_box(face, min, max);
    let corners = face.quad_box(min, max);
    let shade_idx = face.shade_idx();
    let wh = ((w - 1) & 0xF) | (((h - 1) & 0xF) << 4);
    let start = opaque.len() as u32;
    for (corner, p) in corners.into_iter().enumerate() {
        opaque.push(Vertex {
            pos: p,
            tint: key.tint,
            packed: pack_vertex(
                key.tile,
                corner as u32,
                shade_idx,
                wh,
                false,
                key.ao,
                key.light6,
            ) | (UV_MODE_NONE << UV_MODE_SHIFT),
            packed2: pack_vertex2(key.block6),
        });
    }
    opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

#[inline]
fn overlap_greedy_box(face: Face, mut min: [f32; 3], mut max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (_, u, v) = face_axes(face);
    min[u] -= GREEDY_FACE_OVERLAP;
    max[u] += GREEDY_FACE_OVERLAP;
    min[v] -= GREEDY_FACE_OVERLAP;
    max[v] += GREEDY_FACE_OVERLAP;
    (min, max)
}

/// Emit an X-shaped plant: two diagonal billboard quads into the opaque (cutout)
/// buffer, each drawn in BOTH windings so the plant is visible from both sides
/// under back-face culling. Flat-lit (AO = 3, shade index 0 = "top", no
/// directional darkening), biome-tinted for grass/fern; `fs_opaque`'s alpha
/// discard handles the transparent texels exactly like leaves.
fn emit_cross(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    bx: f32,
    y: f32,
    bz: f32,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
) {
    // Flat-lit: shade index 0 (top, no directional darkening), AO = 3, no overlay;
    // `pack_vertex`/`pack_vertex2` own the bit layouts.
    for plane in cross_quads(bx, y, bz) {
        let start = opaque.len() as u32;
        for (corner, p) in plane.into_iter().enumerate() {
            opaque.push(Vertex {
                pos: p,
                tint,
                packed: pack_vertex(tile.index() as u32, corner as u32, 0, 0, false, 3, sky6),
                packed2: pack_vertex2(block6),
            });
        }
        opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        opaque_idx.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    }
}

#[cfg(test)]
mod fold_light_tests {
    use super::*;

    /// The light-channel split's terrain identity: per-channel quantization is
    /// monotone, so `max(sky6, block6)` reproduces the pre-split single channel
    /// (`quantize(max(sums))`) exactly — the shader's `max(sky_term, block_term)`
    /// at identity scale therefore matches the old fold bit-for-bit. Also pins
    /// `fold_light_smooth`'s constant-divisor arms to `fold_light` byte parity.
    #[test]
    fn split_channels_reproduce_the_max_folded_single_channel() {
        for cnt in 1u32..=4 {
            let denom = cnt * SKY_FULL as u32;
            for sky in 0..=denom {
                for blk in 0..=denom {
                    let (s6, b6, warm) = fold_light(sky, blk, denom);
                    let old = ((sky.max(blk) * 63 + denom / 2) / denom).min(63);
                    assert_eq!(s6.max(b6), old, "sky={sky} blk={blk} denom={denom}");
                    let smooth = fold_light_smooth(sky, blk, cnt);
                    assert_eq!(
                        smooth,
                        (s6, b6, warm),
                        "smooth arm must stay byte-identical at cnt={cnt}"
                    );
                }
            }
        }
    }
}
