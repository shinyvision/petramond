use petramond_world::block::{Block, ShapeState};
use petramond_world::chunk::{SECTION_SIZE, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::water_math;

pub(super) const SECTION_PAD: usize = SECTION_SIZE + 2;
/// [`SECTION_PAD`] under the name the mesh module re-exports.
pub(in crate) const MESH_PAD_SIDE: usize = SECTION_PAD;
const BIOME_PAD_RADIUS: i32 = 2;
const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);

#[inline]
pub(in crate) fn mesh_pad_idx(x: usize, y: usize, z: usize) -> usize {
    (y * SECTION_PAD + z) * SECTION_PAD + x
}

#[inline]
fn biome_pad_idx(x: usize, z: usize) -> usize {
    z * BIOME_PAD + x
}

pub struct SectionMeshPad<'a> {
    pub blocks: &'a [u16],
    pub water: &'a [u8],
    pub skylight: &'a [u8],
    /// Per-cell block light, packed RGB — the mesher averages it PER CHANNEL
    /// and emits all three into the vertex's split light lanes.
    pub blocklight: &'a [petramond_world::light::LightRgb],
    /// The UNIFIED per-cell block state (opaque; decoded by the owning
    /// family's codec gated on the cell's block).
    pub cell_states: &'a [ShapeState],
    pub loaded: &'a [bool],
    pub biome: &'a [u8],
}

impl SectionMeshPad<'_> {
    #[inline]
    pub(in crate) fn block_at_pad(&self, px: usize, py: usize, pz: usize) -> Block {
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
    pub(super) fn block_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> u16 {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(0, |i| self.blocks[i])
    }

    #[inline]
    pub(super) fn cell_state_world(
        &self,
        ox: i32,
        oy: i32,
        oz: i32,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> ShapeState {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(ShapeState::NONE, |i| self.cell_states[i])
    }

    #[inline]
    pub(super) fn water_world(&self, ox: i32, oy: i32, oz: i32, wx: i32, wy: i32, wz: i32) -> u8 {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(0, |i| self.water[i])
    }

    #[inline]
    pub(super) fn skylight_world(
        &self,
        ox: i32,
        oy: i32,
        oz: i32,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> u8 {
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
    pub(super) fn blocklight_world(
        &self,
        ox: i32,
        oy: i32,
        oz: i32,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> petramond_world::light::LightRgb {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .map_or(petramond_world::light::LightRgb::ZERO, |i| self.blocklight[i])
    }

    #[inline]
    pub(super) fn loaded_world(
        &self,
        ox: i32,
        oy: i32,
        oz: i32,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> bool {
        self.world_idx(ox, oy, oz, wx, wy, wz)
            .is_some_and(|i| self.loaded[i])
    }

    #[inline]
    pub(super) fn biome_world(&self, ox: i32, oz: i32, wx: i32, wz: i32) -> u8 {
        let (px, pz) = (wx - (ox - BIOME_PAD_RADIUS), wz - (oz - BIOME_PAD_RADIUS));
        let n = BIOME_PAD as i32;
        if (0..n).contains(&px) && (0..n).contains(&pz) {
            self.biome[biome_pad_idx(px as usize, pz as usize)]
        } else {
            0
        }
    }

    /// Pad-local water probes for in-section cells and their ±1 neighbours.
    /// The neighbour-above sample for `fills_cell` can sit one cell past the
    /// top pad face — that matches `block_world` returning air out of pad.
    #[inline]
    fn local_pad_xyz(lx: i32, ly: i32, lz: i32) -> Option<(usize, usize, usize)> {
        let n = SECTION_PAD as i32;
        let (px, py, pz) = (lx + 1, ly + 1, lz + 1);
        if (0..n).contains(&px) && (0..n).contains(&py) && (0..n).contains(&pz) {
            Some((px as usize, py as usize, pz as usize))
        } else {
            None
        }
    }

    #[inline]
    fn block_above_local(&self, px: usize, py: usize, pz: usize) -> Block {
        if py + 1 < SECTION_PAD {
            Block::from_id(self.blocks[mesh_pad_idx(px, py + 1, pz)])
        } else {
            Block::Air
        }
    }

    #[inline]
    pub(super) fn water_fills_local(&self, lx: i32, ly: i32, lz: i32) -> bool {
        let Some((px, py, pz)) = Self::local_pad_xyz(lx, ly, lz) else {
            return false;
        };
        let i = mesh_pad_idx(px, py, pz);
        if Block::from_id(self.blocks[i]) != Block::Water {
            return false;
        }
        water_math::fills_cell(self.water[i], self.block_above_local(px, py, pz))
    }

    #[inline]
    pub(super) fn fluid_height_local(&self, lx: i32, ly: i32, lz: i32) -> Option<f32> {
        let (px, py, pz) = Self::local_pad_xyz(lx, ly, lz)?;
        let i = mesh_pad_idx(px, py, pz);
        if Block::from_id(self.blocks[i]) != Block::Water {
            return None;
        }
        Some(water_math::fluid_height(
            self.water[i],
            self.block_above_local(px, py, pz),
        ))
    }

    #[inline]
    pub(super) fn water_still_local(&self, lx: i32, ly: i32, lz: i32) -> bool {
        let Some((px, py, pz)) = Self::local_pad_xyz(lx, ly, lz) else {
            return false;
        };
        let i = mesh_pad_idx(px, py, pz);
        Block::from_id(self.blocks[i]) == Block::Water && water_math::is_still_source(self.water[i])
    }

    #[inline]
    pub(super) fn block_local(&self, lx: i32, ly: i32, lz: i32) -> Block {
        Self::local_pad_xyz(lx, ly, lz)
            .map(|(px, py, pz)| self.block_at_pad(px, py, pz))
            .unwrap_or(Block::Air)
    }
}
