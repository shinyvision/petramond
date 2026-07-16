use glam::{IVec3, Vec3};

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
use crate::block_model::{self, BlockModelKind};
use crate::block_state::{LogAxis, SlabState, StairState};
use crate::chunk::{
    section_idx, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y,
};
use crate::facing::Facing;
use crate::section::Section;
use crate::torch::warm_tint;

use super::face::{cactus_quad, crop_quads, cross_quads, quad_for, vertex_ao, Face, FACES};
use super::tint;
use super::vertex::{
    pack_tint, pack_vertex, pack_vertex2, ChunkMesh, ModelVertex, Vertex, UV_MODE_NONE,
};
use super::water::{self, SideVsWater, WaterSurface};

use super::face_emit::{
    cube_face_lighting_pad, fold_light, fold_light_smooth, push_cube_face_with_cell_uvs,
    slab_corner_open,
};
use super::greedy::{emit_greedy_quads, FlatFace, GreedyScratch, GREEDY};

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

#[inline]
fn cube_face_tile(
    block: Block,
    face: Face,
    tiles: [Tile; 3],
    furnace_faces: Option<(Face, Tile)>,
    log_axis: LogAxis,
) -> Tile {
    let [tile_top, tile_bot, tile_side] = tiles;
    if block.is_log() {
        return match (log_axis, face) {
            (LogAxis::X, Face::PosX) | (LogAxis::Y, Face::PosY) | (LogAxis::Z, Face::PosZ) => {
                tile_top
            }
            (LogAxis::X, Face::NegX) | (LogAxis::Y, Face::NegY) | (LogAxis::Z, Face::NegZ) => {
                tile_bot
            }
            _ => tile_side,
        };
    }
    match face {
        Face::PosY => tile_top,
        Face::NegY => tile_bot,
        _ => match furnace_faces {
            Some((front_face, front_tile)) if face == front_face => front_tile,
            Some(_) => crate::atlas::engine().furnace_side,
            None => tile_side,
        },
    }
}

#[inline]
fn uv_16ths(value: f32) -> u32 {
    (value.clamp(0.0, 1.0) * 16.0).round() as u32
}

#[inline]
fn log_side_cell_uvs(
    axis: LogAxis,
    face: Face,
    corners: [[f32; 3]; 4],
    base: [f32; 3],
) -> Option<[(u32, u32); 4]> {
    let mut uvs = [(0, 0); 4];
    for (i, corner) in corners.into_iter().enumerate() {
        let local = [
            corner[0] - base[0],
            corner[1] - base[1],
            corner[2] - base[2],
        ];
        let [u, v] = face.log_side_cell_uv(axis, local)?;
        uvs[i] = (uv_16ths(u), uv_16ths(v));
    }
    Some(uvs)
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

#[inline]
pub(super) fn mesh_pad_idx(x: usize, y: usize, z: usize) -> usize {
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
    pub stair_states: &'a [u8],
    pub slab_states: &'a [SlabState],
    pub loaded: &'a [bool],
    pub biome: &'a [u8],
}

impl SectionMeshPad<'_> {
    #[inline]
    pub(super) fn block_at_pad(&self, px: usize, py: usize, pz: usize) -> Block {
        Block::from_id(self.blocks[mesh_pad_idx(px, py, pz)])
    }

    /// A slab cell with BOTH halves filled renders as a full block: it culls
    /// adjacent faces and occludes AO/light exactly like an opaque cube (the
    /// closure paths make the same test through `neighbour_slab_state`).
    #[inline]
    pub(super) fn full_slab_stack_at_pad(
        &self,
        block: Block,
        px: usize,
        py: usize,
        pz: usize,
    ) -> bool {
        block.is_slab() && self.slab_states[mesh_pad_idx(px, py, pz)].is_full()
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
    fn stair_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> StairState {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(StairState::default(), |i| {
                StairState::decode(self.stair_states[i])
            })
    }

    #[inline]
    fn slab_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> SlabState {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(SlabState::EMPTY, |i| self.slab_states[i])
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
    // Glass stays on the per-face path: its glass-vs-glass cull (interior faces
    // of a glass wall) isn't representable in the opaque-rows exposure masks.
    // Translucent blocks (ice) stay there too — same-block cull plus the
    // alpha-blended buffer, neither of which the fast path emits.
    block != Block::Water
        && block != Block::Cactus
        && block != Block::Glass
        && !block.is_translucent()
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
                let block = pad.block_at_pad(px, py, pz);
                if block.is_opaque() || pad.full_slab_stack_at_pad(block, px, py, pz) {
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
                if block == Block::Air {
                    continue;
                }
                // Same-material full slab stacks take the cube fast path too; this
                // MUST match the slab-branch fall-through in `section_geometry`.
                let slab_as_cube = block.is_slab()
                    && crate::slab::is_uniform_full_stack(
                        pad.slab_states[mesh_pad_idx(lx + 1, ly + 1, lz + 1)],
                    );
                if !pad_cube_fast_candidate(block) && !slab_as_cube {
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
    neighbour_stair_state: impl Fn(i32, i32, i32) -> StairState,
    neighbour_slab_state: impl Fn(i32, i32, i32) -> SlabState,
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
        &neighbour_stair_state,
        &neighbour_slab_state,
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
        &neighbour_stair_state,
        &neighbour_slab_state,
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
    let nb_stair_state = |wx, wy, wz| pad.stair_world(ox, oy, oz, wx, wy, wz);
    let nb_slab_state = |wx, wy, wz| pad.slab_world(ox, oy, oz, wx, wy, wz);
    let nb_water = |wx, wy, wz| pad.water_world(ox, oy, oz, wx, wy, wz);
    let nb_biome = |wx, wz| pad.biome_world(ox, oz, wx, wz);
    let nb_skylight = |wx, wy, wz| pad.skylight_world(ox, oy, oz, wx, wy, wz);
    let nb_blocklight = |wx, wy, wz| pad.blocklight_world(ox, oy, oz, wx, wy, wz);
    let nb_loaded = |wx, wy, wz| pad.loaded_world(ox, oy, oz, wx, wy, wz);
    let tints = section
        .has_biome_tint_blocks()
        .then(|| tint::biome_window(ox, oz, nb_biome));
    let mut mesh = section_geometry(
        section,
        pos,
        nb_block,
        nb_stair_state,
        nb_slab_state,
        nb_water,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
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
        nb_block,
        nb_stair_state,
        nb_slab_state,
        nb_water,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
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
    neighbour_stair_state: impl Fn(i32, i32, i32) -> StairState,
    neighbour_slab_state: impl Fn(i32, i32, i32) -> SlabState,
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
    let mut translucent = vec![];
    let mut translucent_idx = vec![];
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
    let slab_at = |wx: i32, wy: i32, wz: i32| -> Option<SlabState> {
        let block = block_at(wx, wy, wz);
        crate::slab::is_slab(block)
            .then(|| crate::slab::normalize_state(block, neighbour_slab_state(wx, wy, wz)))
    };
    // "Cell holds a full slab stack" — callers gate on `is_slab` first (dense flag)
    // so this only pays a state lookup on actual slab cells. Full stacks cull and
    // occlude AO/light like opaque cubes; no normalize needed (a normalized default
    // is a single layer, never full).
    let slab_full_at =
        |wx: i32, wy: i32, wz: i32| -> bool { neighbour_slab_state(wx, wy, wz).is_full() };
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 { neighbour_water(wx, wy, wz) };
    let fluid_at = |wx: i32, wy: i32, wz: i32| -> Option<f32> {
        if block_at(wx, wy, wz) != Block::Water {
            return None;
        }
        Some(crate::world::water::fluid_height(
            water_at(wx, wy, wz),
            block_at(wx, wy + 1, wz),
        ))
    };
    let water_fills_cell = |wx: i32, wy: i32, wz: i32| -> bool {
        if block_at(wx, wy, wz) != Block::Water {
            return false;
        }
        crate::world::water::fills_cell(water_at(wx, wy, wz), block_at(wx, wy + 1, wz))
    };
    // Still-source probe for the flow gradient: two adjacent still sources
    // never flow into each other (see `water::surface_flow_dir`).
    let water_still_at = |wx: i32, wy: i32, wz: i32| -> bool {
        block_at(wx, wy, wz) == Block::Water
            && crate::world::water::is_still_source(water_at(wx, wy, wz))
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

                if matches!(shape, RenderShape::Cross | RenderShape::Crop) {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tint_tile(tile.world_tint(), ci), warm);
                    emit_plant(
                        &mut opaque,
                        &mut opaque_idx,
                        shape,
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

                if shape == RenderShape::Ladder {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let facing = section.entity_facing(lx, ly, lz);
                    super::ladder::emit_ladder_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        facing,
                        tile,
                        tint_tile(tile.world_tint(), ci),
                        sky6,
                        block6,
                        warm,
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
                    let state = section.stair_state(lx, ly, lz);
                    let shape = crate::stair::resolved_shape(IVec3::new(wx, wy, wz), state, |p| {
                        crate::stair::is_stair(block_at(p.x, p.y, p.z))
                            .then(|| neighbour_stair_state(p.x, p.y, p.z))
                    });
                    super::stair::emit_stair_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        shape,
                        [tile_top, tile_bot, tile_side],
                        &tint_for,
                        &block_at,
                        &slab_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    continue;
                }

                if shape == RenderShape::Pane {
                    // [top, bottom, side] tiles = [edge, edge, glass].
                    let [edge_tile, _bottom, glass_tile] = block.tiles();
                    // A neighbour stair's resolved corner shape decides whether its
                    // face toward the pane is complete — same neighbour-of-neighbour
                    // read the stair's own corner resolution does.
                    let stair_shape_at = |q: IVec3| {
                        crate::stair::resolved_shape(q, neighbour_stair_state(q.x, q.y, q.z), |r| {
                            crate::stair::is_stair(block_at(r.x, r.y, r.z))
                                .then(|| neighbour_stair_state(r.x, r.y, r.z))
                        })
                    };
                    let pane_mask_at = |p: IVec3| {
                        crate::pane::resolved_mask(
                            p,
                            |q| block_at(q.x, q.y, q.z),
                            &stair_shape_at,
                            |q| slab_full_at(q.x, q.y, q.z),
                        )
                    };
                    let vertical = |dy: i32| {
                        let vb = block_at(wx, wy + dy, wz);
                        if vb == block {
                            super::pane::PaneVertical::Pane(pane_mask_at(IVec3::new(
                                wx,
                                wy + dy,
                                wz,
                            )))
                        } else if vb.is_opaque() || (vb.is_slab() && slab_full_at(wx, wy + dy, wz))
                        {
                            super::pane::PaneVertical::Solid
                        } else {
                            super::pane::PaneVertical::Open
                        }
                    };
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    super::pane::emit_pane_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        pane_mask_at(IVec3::new(wx, wy, wz)),
                        vertical(1),
                        vertical(-1),
                        glass_tile,
                        edge_tile,
                        tint_tile(glass_tile.world_tint(), ci),
                        sky6,
                        block6,
                        warm,
                    );
                    continue;
                }

                // A same-material full slab stack IS the material's full cube: fall
                // through to the cube path (fast path + greedy merge included) so it
                // culls, lights, and merges like one. Partial cells and mixed-material
                // full stacks keep the per-layer emitter (preserving each layer's
                // texture); full stacks of either kind still cull/occlude as opaque
                // via `slab_full_at`.
                let mut slab_as_cube = false;
                if shape == RenderShape::Slab {
                    let state = crate::slab::normalize_state(block, section.slab_state(lx, ly, lz));
                    slab_as_cube = crate::slab::is_uniform_full_stack(state);
                    if !slab_as_cube {
                        let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                        super::slab::emit_slab_block(
                            &mut opaque,
                            &mut opaque_idx,
                            wx,
                            wy,
                            wz,
                            state,
                            &tint_for,
                            &block_at,
                            &slab_at,
                            &neighbour_light,
                            &neighbour_blocklight,
                        );
                        continue;
                    }
                }

                let is_water = block == Block::Water;
                let block_tiles = block.tiles();
                // Grass sides swap to the untinted snowy texture while a
                // snow-cover block (snow layer / snow block) sits directly on
                // top — derived from the neighbour above at mesh time, so it
                // heals itself the moment the cover is placed or dug.
                let grass_snow_covered =
                    block == Block::Grass && block_at(wx, wy + 1, wz).is_snow_cover();
                let log_axis = if block.is_log() {
                    section.log_axis(lx, ly, lz)
                } else {
                    LogAxis::Y
                };
                let furnace_faces = (block == Block::Furnace).then(|| {
                    let front = if section.is_furnace_lit(lx, ly, lz) {
                        crate::atlas::engine().furnace_front_on
                    } else {
                        crate::atlas::engine().furnace_front
                    };
                    (facing_face(section.entity_facing(lx, ly, lz)), front)
                });
                let base_x = wx as f32;
                let base_z = wz as f32;
                let base_y = wy as f32;

                let water_surface = is_water.then(|| {
                    let full = water_fills_cell(wx, wy, wz);
                    WaterSurface::new(wx, wy, wz, full, &block_at, &fluid_at, &water_still_at)
                });

                if let (Some(pad), Some(exposed)) = (pad, exposed_masks.as_ref()) {
                    if pad_cube_fast_candidate(block) || slab_as_cube {
                        let cell = section_idx(lx, ly, lz);
                        for face in FACES {
                            if !mask_has(exposed, face, cell) {
                                continue;
                            }
                            let is_side =
                                matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                            let (base_tile, overlay_tile, tint) =
                                if block == Block::Grass && is_side {
                                    let e = crate::atlas::engine();
                                    if grass_snow_covered {
                                        (e.grass_snow, None, tint_tile(e.grass_snow.world_tint(), ci))
                                    } else {
                                        (e.dirt, Some(e.grass_side_overlay), tint_grass(ci))
                                    }
                                } else {
                                    let t = cube_face_tile(
                                        block,
                                        face,
                                        block_tiles,
                                        furnace_faces,
                                        log_axis,
                                    );
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
                            let log_uvs = log_side_cell_uvs(
                                log_axis,
                                face,
                                corners,
                                [base_x, base_y, base_z],
                            );
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
                            if overlay_tile.is_none()
                                && (block.is_opaque() || slab_as_cube)
                                && flat
                                && log_uvs.is_none()
                            {
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
                                    tint: pack_tint(final_tint),
                                };
                                let s = [lx, ly, lz][face_axes(face).0];
                                greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                            } else {
                                push_cube_face_with_cell_uvs(
                                    &mut opaque,
                                    &mut opaque_idx,
                                    corners,
                                    base_tile,
                                    overlay,
                                    has_overlay,
                                    UV_MODE_NONE,
                                    log_uvs,
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
                    // A lowered cube's top plane sits INSIDE the cell — nothing
                    // above can ever cover it, so it is exempt from the
                    // neighbour cull (the block-above's bottom face still draws
                    // since lowered rows are non-opaque: no x-ray slit).
                    let lowered = match shape {
                        RenderShape::LoweredCube(h) => Some(h),
                        _ => None,
                    };
                    let is_lowered_top = lowered.is_some() && matches!(face, Face::PosY);
                    let nb_solid = nb.is_opaque() || (nb.is_slab() && slab_full_at(nwx, nwy, nwz));
                    if nb_solid && !is_water_top && !is_cactus_side && !is_lowered_top {
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
                    // Two adjacent glass blocks share no visible face: cull both
                    // sides so a glass wall reads as one pane, not stacked frames.
                    if block == Block::Glass && nb == Block::Glass {
                        continue;
                    }
                    // Same rule for a translucent block against itself (ice
                    // against ice): interior faces would double-blend, so the
                    // frozen sheet reads as one volume, not stacked slabs.
                    if block.is_translucent() && nb == block {
                        continue;
                    }
                    // Two flush lowered cubes share no visible side either: the
                    // neighbour's body covers my whole (equally short) face.
                    if let (Some(h), RenderShape::LoweredCube(nh)) = (lowered, nb.render_shape()) {
                        if is_side && nh >= h {
                            continue;
                        }
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
                            // A STILL SOURCE's side faces are calm water — the
                            // step walls of the recessed pocket under a block
                            // sitting in the sea must not stream. Flowing and
                            // falling cells keep the animated flow sides.
                            _ if water_still_at(wx, wy, wz) => {
                                crate::atlas::engine().water_still
                            }
                            _ => crate::atlas::engine().water_flow,
                        };
                        (t, None, tint_water(ci))
                    } else if block == Block::Grass && is_side {
                        let e = crate::atlas::engine();
                        if grass_snow_covered {
                            (e.grass_snow, None, tint_tile(e.grass_snow.world_tint(), ci))
                        } else {
                            (e.dirt, Some(e.grass_side_overlay), tint_grass(ci))
                        }
                    } else {
                        let t = cube_face_tile(block, face, block_tiles, furnace_faces, log_axis);
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
                    if let Some(h) = lowered {
                        // Sink the visible top: the top face drops to h/16 and
                        // side faces shorten with it (full tile compressed a
                        // texel, like the cactus insets — no UV plumbing).
                        let top = base_y + h as f32 / 16.0;
                        for c in &mut corners {
                            c[1] = c[1].min(top);
                        }
                    }
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
                    let log_uvs =
                        log_side_cell_uvs(log_axis, face, corners, [base_x, base_y, base_z]);

                    let (ao, light6, block6, warm) = cube_face_lighting(
                        face,
                        fx,
                        fy,
                        fz,
                        f_l,
                        f_bl,
                        true,
                        &block_at,
                        &slab_at,
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
                    if !is_water
                        && overlay_tile.is_none()
                        && (block.is_opaque() || slab_as_cube)
                        && flat
                        && log_uvs.is_none()
                    {
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
                            tint: pack_tint(final_tint),
                        };
                        // Slice index = the cell's coord along this face's normal axis.
                        let s = [lx, ly, lz][face_axes(face).0];
                        greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                    } else {
                        // Translucent blocks (ice) blend in their own
                        // depth-writing pass; their texels sit below the
                        // opaque pass's cutout and would discard to nothing
                        // there, and water's read-only depth cannot resolve a
                        // translucent cube sheet's own face order.
                        let (vbuf, ibuf) = if is_water {
                            (&mut transparent, &mut transparent_idx)
                        } else if block.is_translucent() {
                            (&mut translucent, &mut translucent_idx)
                        } else {
                            (&mut opaque, &mut opaque_idx)
                        };
                        let tris = push_cube_face_with_cell_uvs(
                            vbuf,
                            ibuf,
                            corners,
                            base_tile,
                            overlay,
                            has_overlay,
                            UV_MODE_NONE,
                            log_uvs,
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
        translucent,
        translucent_idx,
        model,
        model_idx,
        mesh_dirty: true,
        ..ChunkMesh::empty()
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
pub(super) fn cube_face_lighting<B, S, L, K>(
    face: Face,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) -> ([u32; 4], [u32; 4], [u32; 4], [f32; 4])
where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();

    let mut occ = [[false; 3]; 3];
    let mut opq = [[false; 3]; 3];
    let mut sky = [[0u32; 3]; 3];
    let mut blk = [[0u32; 3]; 3];
    let mut slab = [[SlabState::EMPTY; 3]; 3];
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
            // A full slab stack occludes AO and carries no light, exactly like an
            // opaque cube — without this it darkens corners twice (it blocks the
            // light flood, then still enters the smooth-light mean as a dark open
            // cell). Partial slab states are kept for the per-corner octant gate
            // below. The dense `is_slab` flag gates the state lookup.
            let slab_state = if cell.is_slab() {
                slab_at(cx, cy, cz)
            } else {
                None
            };
            let full_stack = slab_state.is_some_and(|s| s.is_full());
            occ[ia][ib] = cell.occludes_ao() || full_stack;
            if smooth_light {
                opq[ia][ib] = cell.is_opaque() || full_stack;
                if !opq[ia][ib] {
                    sky[ia][ib] = neighbour_light(cx, cy, cz) as u32;
                    blk[ia][ib] = neighbour_blocklight(cx, cy, cz) as u32;
                    if let Some(state) = slab_state {
                        slab[ia][ib] = state;
                    }
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
        for (ia, ib, a, b) in [(iu, 1, su, 0), (1, iv, 0, sv), (iu, iv, su, sv)] {
            if opq[ia][ib] || !slab_corner_open(slab[ia][ib], face, a, b, su, sv) {
                continue;
            }
            sum += sky[ia][ib];
            sum_block += blk[ia][ib];
            cnt += 1;
        }
        (light6[corner], block6[corner], warm[corner]) = fold_light_smooth(sum, sum_block, cnt);
    }
    (ao, light6, block6, warm)
}

/// A cube face's `(normal, U, V)` local axes (0=X, 1=Y, 2=Z), derived from `Face::quad_box`
/// so the greedy slice's `(u,v)` grid and a merged quad's tiled UV (W tiles along U, H along
/// V) align with `corner_local`: normal-X → U=Z,V=Y; normal-Y → U=X,V=Z; normal-Z → U=X,V=Y.
#[inline]
pub(super) fn face_axes(face: Face) -> (usize, usize, usize) {
    match face {
        Face::PosX | Face::NegX => (0, 2, 1),
        Face::PosY | Face::NegY => (1, 0, 2),
        Face::PosZ | Face::NegZ => (2, 0, 1),
    }
}

/// Index of a face in [`FACES`] — the per-direction plane in [`GreedyScratch::faces`]. Must
/// match `FACES.into_iter().enumerate()` in [`emit_greedy_quads`].
#[inline]
pub(super) fn face_index(face: Face) -> usize {
    match face {
        Face::PosX => 0,
        Face::NegX => 1,
        Face::PosY => 2,
        Face::NegY => 3,
        Face::PosZ => 4,
        Face::NegZ => 5,
    }
}

/// Emit a billboard plant — the X cross (two diagonal quads) or the planted
/// crop lattice (four axis-aligned quads, see `crop_quads`) — into the opaque
/// (cutout) buffer, each plane drawn in BOTH windings so the plant is visible
/// from both sides under back-face culling. Flat-lit (AO = 3, shade index 0 =
/// "top", no directional darkening), biome-tinted for grass/fern;
/// `fs_opaque`'s alpha discard handles the transparent texels exactly like
/// leaves.
fn emit_plant(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    shape: RenderShape,
    bx: f32,
    y: f32,
    bz: f32,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
) {
    let cross;
    let crop;
    let planes: &[[[f32; 3]; 4]] = if shape == RenderShape::Crop {
        crop = crop_quads(bx, y, bz);
        &crop
    } else {
        cross = cross_quads(bx, y, bz);
        &cross
    };
    // Flat-lit: shade index 0 (top, no directional darkening), AO = 3, no overlay;
    // `pack_vertex`/`pack_vertex2` own the bit layouts.
    for plane in planes {
        let start = opaque.len() as u32;
        for (corner, p) in plane.iter().enumerate() {
            opaque.push(Vertex {
                pos: *p,
                tint: pack_tint(tint),
                packed: pack_vertex(tile.index() as u32, corner as u32, 0, 0, false, 3, sky6),
                packed2: pack_vertex2(block6),
            });
        }
        opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        opaque_idx.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    }
}
