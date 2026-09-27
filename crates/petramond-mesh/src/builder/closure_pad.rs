use glam::IVec3;
use petramond_world::block::{snow_cover_at, Block, ShapeState, SNOW_COVER_REACH};
use petramond_world::chunk::{SectionPos, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::light::LightRgb;

use super::pad::{
    biome_pad_idx, mesh_pad_idx, SectionMeshPad, BIOME_PAD, BIOME_PAD_RADIUS, SECTION_PAD,
};

pub struct WorldReads<'r> {
    pub block: &'r dyn Fn(i32, i32, i32) -> u16,
    pub cell_state: &'r dyn Fn(i32, i32, i32) -> ShapeState,
    pub fluid_meta: &'r dyn Fn(i32, i32, i32) -> u8,
    pub biome: &'r dyn Fn(i32, i32) -> u8,
    pub skylight: &'r dyn Fn(i32, i32, i32) -> u8,
    pub blocklight: &'r dyn Fn(i32, i32, i32) -> LightRgb,
    pub loaded: &'r dyn Fn(i32, i32, i32) -> bool,
    pub dyed: &'r dyn Fn(i32, i32, i32) -> bool,
}

pub(super) struct ClosurePad {
    blocks: Vec<u16>,
    fluid: Vec<u8>,
    skylight: Vec<u8>,
    blocklight: Vec<LightRgb>,
    cell_states: Vec<ShapeState>,
    loaded: Vec<bool>,
    transition_blocked: Vec<bool>,
    biome: Vec<u8>,
}

impl ClosurePad {
    pub(super) fn assemble(pos: SectionPos, reads: &WorldReads<'_>) -> Self {
        const PAD_VOL: usize = SECTION_PAD * SECTION_PAD * SECTION_PAD;
        let (ox, oy, oz) = pos.origin_world();
        let mut pad = Self {
            blocks: vec![0; PAD_VOL],
            fluid: vec![0; PAD_VOL],
            skylight: vec![SKY_FULL; PAD_VOL],
            blocklight: vec![LightRgb::ZERO; PAD_VOL],
            cell_states: vec![ShapeState::NONE; PAD_VOL],
            loaded: vec![false; PAD_VOL],
            transition_blocked: vec![false; PAD_VOL],
            biome: vec![0; BIOME_PAD * BIOME_PAD],
        };
        for py in 0..SECTION_PAD {
            for pz in 0..SECTION_PAD {
                for px in 0..SECTION_PAD {
                    let (wx, wy, wz) = (ox - 1 + px as i32, oy - 1 + py as i32, oz - 1 + pz as i32);
                    let i = mesh_pad_idx(px, py, pz);
                    pad.blocks[i] = (reads.block)(wx, wy, wz);
                    pad.fluid[i] = (reads.fluid_meta)(wx, wy, wz);
                    pad.skylight[i] = if wy >= WORLD_MAX_Y {
                        SKY_FULL
                    } else if wy < WORLD_MIN_Y {
                        0
                    } else {
                        (reads.skylight)(wx, wy, wz)
                    };
                    pad.blocklight[i] = (reads.blocklight)(wx, wy, wz);
                    pad.cell_states[i] = (reads.cell_state)(wx, wy, wz);
                    pad.loaded[i] = (reads.loaded)(wx, wy, wz);
                    pad.transition_blocked[i] = (reads.dyed)(wx, wy, wz)
                        || snow_cover_at(IVec3::new(wx, wy + SNOW_COVER_REACH, wz), |p| {
                            Block::from_id((reads.block)(p.x, p.y, p.z))
                        })
                        .is_some();
                }
            }
        }
        for pz in 0..BIOME_PAD {
            for px in 0..BIOME_PAD {
                pad.biome[biome_pad_idx(px, pz)] = (reads.biome)(
                    ox - BIOME_PAD_RADIUS + px as i32,
                    oz - BIOME_PAD_RADIUS + pz as i32,
                );
            }
        }
        pad
    }

    pub(super) fn view(&self) -> SectionMeshPad<'_> {
        SectionMeshPad {
            table: petramond_world::block::BlockTable::current(),
            blocks: &self.blocks,
            fluid: &self.fluid,
            skylight: &self.skylight,
            blocklight: &self.blocklight,
            cell_states: &self.cell_states,
            transition_blocked: &self.transition_blocked,
            loaded: &self.loaded,
            biome: &self.biome,
        }
    }
}
