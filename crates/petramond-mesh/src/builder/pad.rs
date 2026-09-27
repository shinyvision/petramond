use petramond_world::block::{Block, BlockTable, ShapeState};
use petramond_world::chunk::SECTION_SIZE;
use petramond_world::fluid_math;

pub(super) const SECTION_PAD: usize = SECTION_SIZE + 2;
pub(super) const BIOME_PAD_RADIUS: i32 = 2;
pub(super) const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);

#[inline]
pub(crate) fn mesh_pad_idx(x: usize, y: usize, z: usize) -> usize {
    (y * SECTION_PAD + z) * SECTION_PAD + x
}

#[inline]
pub(super) fn biome_pad_idx(x: usize, z: usize) -> usize {
    z * BIOME_PAD + x
}

pub struct SectionMeshPad<'a> {
    pub table: BlockTable,
    pub blocks: &'a [u16],
    pub fluid: &'a [u8],
    pub skylight: &'a [u8],
    pub blocklight: &'a [petramond_world::light::LightRgb],
    pub cell_states: &'a [ShapeState],
    pub transition_blocked: &'a [bool],
    pub loaded: &'a [bool],
    pub biome: &'a [u8],
}

impl SectionMeshPad<'_> {
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
            self.table.block(self.blocks[mesh_pad_idx(px, py + 1, pz)])
        } else {
            Block::Air
        }
    }

    #[inline]
    pub(super) fn fluid_fills_local(&self, lx: i32, ly: i32, lz: i32, fluid: Block) -> bool {
        let Some((px, py, pz)) = Self::local_pad_xyz(lx, ly, lz) else {
            return false;
        };
        let i = mesh_pad_idx(px, py, pz);
        if self.table.fluid(self.blocks[i]) != Some(fluid) {
            return false;
        }
        fluid_math::fills_cell(self.fluid[i], self.block_above_local(px, py, pz), fluid)
    }

    #[inline]
    pub(super) fn fluid_height_local(
        &self,
        lx: i32,
        ly: i32,
        lz: i32,
        fluid: Block,
    ) -> Option<f32> {
        let (px, py, pz) = Self::local_pad_xyz(lx, ly, lz)?;
        let i = mesh_pad_idx(px, py, pz);
        if self.table.fluid(self.blocks[i]) != Some(fluid) {
            return None;
        }
        Some(fluid_math::fluid_height(
            self.fluid[i],
            self.block_above_local(px, py, pz),
            fluid,
        ))
    }

    #[inline]
    pub(super) fn fluid_still_local(&self, lx: i32, ly: i32, lz: i32, fluid: Block) -> bool {
        let Some((px, py, pz)) = Self::local_pad_xyz(lx, ly, lz) else {
            return false;
        };
        let i = mesh_pad_idx(px, py, pz);
        self.table.fluid(self.blocks[i]) == Some(fluid)
            && fluid_math::is_still_source(self.fluid[i])
    }

    #[inline]
    pub(super) fn fluid_falling_local(&self, lx: i32, ly: i32, lz: i32, fluid: Block) -> bool {
        let Some((px, py, pz)) = Self::local_pad_xyz(lx, ly, lz) else {
            return false;
        };
        let i = mesh_pad_idx(px, py, pz);
        self.table.fluid(self.blocks[i]) == Some(fluid) && fluid_math::is_falling(self.fluid[i])
    }
}
