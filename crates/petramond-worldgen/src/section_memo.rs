//! A section's terrain before features — filled and carved — memoized
//! process-wide, because two consumers need exactly it: section generation,
//! and the positional terrain queries a pack asks while placing content that
//! spans sections. A pack probing a cell's floor shortly before its sections
//! stream, or dressing a section just after, otherwise paid the fill and the
//! carve twice. Solidity is kept separately and longer than the blocks: it is
//! sixteen times smaller and is what the queries read.

use std::sync::{Arc, LazyLock};

use petramond_world::block::Block;
use petramond_world::chunk::{section_idx, SectionPos, SECTION_SIZE};
use petramond_world::section::{BlockCube, Section};

use crate::density::surface::SurfaceDensitySystem;
use crate::memo::SharedMemo;
use crate::noise::cave_field::{CaveField, FallCell};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    seed: u32,
    tables: [usize; 2],
    pos: [i32; 3],
}

/// Two bit planes per section-local cell index: what the terrain leaves
/// there. `Air` is neither bit — the common case, and the one a reader
/// usually wants.
#[derive(Clone, Copy)]
pub(crate) struct SpaceMask {
    solid: [u64; 64],
    fluid: [u64; 64],
}

impl SpaceMask {
    #[inline]
    pub(crate) fn at(&self, cell: usize) -> mod_api::TerrainSpace {
        let bit = 1 << (cell % 64);
        if self.solid[cell / 64] & bit != 0 {
            mod_api::TerrainSpace::Solid
        } else if self.fluid[cell / 64] & bit != 0 {
            mod_api::TerrainSpace::Fluid
        } else {
            mod_api::TerrainSpace::Air
        }
    }

    #[inline]
    fn set(&mut self, cell: usize, space: mod_api::TerrainSpace) {
        let bit = 1 << (cell % 64);
        self.solid[cell / 64] &= !bit;
        self.fluid[cell / 64] &= !bit;
        match space {
            mod_api::TerrainSpace::Solid => self.solid[cell / 64] |= bit,
            mod_api::TerrainSpace::Fluid => self.fluid[cell / 64] |= bit,
            mod_api::TerrainSpace::Air => {}
        }
    }
}

/// What a generated block leaves a query standing in. Anything that is
/// neither air nor a fluid OCCUPIES the cell, which is what every reader of
/// this asks about.
#[inline]
pub(crate) fn space_of(id: u16) -> mod_api::TerrainSpace {
    let block = Block::from_id(id);
    if block == Block::Air {
        mod_api::TerrainSpace::Air
    } else if block.is_fluid() {
        mod_api::TerrainSpace::Fluid
    } else {
        mod_api::TerrainSpace::Solid
    }
}

static CUBES: LazyLock<SharedMemo<Key, BlockCube>> = LazyLock::new(|| SharedMemo::new(8192));
static SPACES: LazyLock<SharedMemo<Key, Arc<SpaceMask>>> = LazyLock::new(|| SharedMemo::new(16384));

fn key(caves: &CaveField, seed: u32, sp: SectionPos) -> Key {
    Key {
        seed,
        tables: caves.table_identities(),
        pos: [sp.cx, sp.cy, sp.cz],
    }
}

/// The section's filled and carved blocks. `biomes` and `surf` are the
/// column's per-cell biome ids and raw density surfaces, `z*16 + x`.
pub(crate) fn terrain_cube(
    surface: &SurfaceDensitySystem,
    caves: &CaveField,
    seed: u32,
    sp: SectionPos,
    biomes: &[u8],
    surf: &[i32],
) -> BlockCube {
    CUBES.get_or_compute_unlocked(key(caves, seed, sp), || {
        let mut section = Section::new(sp.cx, sp.cy, sp.cz);
        surface.fill_section(&mut section, biomes, surf);
        caves.carve_section(&mut section, surf);
        section.blocks().clone()
    })
}

/// Every cell of section `sp` a fluid fall claims, by section-local index —
/// the one walk the section stamp and the terrain mask share. Falls carry
/// fluid meta an id cube cannot hold, so they are applied after the memo.
pub(crate) fn section_falls(
    caves: &CaveField,
    sp: SectionPos,
    mut visit: impl FnMut(usize, FallCell),
) {
    let (ox, oy, oz) = sp.origin_world();
    if caves.falls_top().is_none_or(|top| oy > top) {
        return;
    }
    let last = SECTION_SIZE as i32 - 1;
    let falls = caves.chunk_falls(sp.cx, sp.cz);
    falls.cells([ox, oy, oz], [ox + last, oy + last, oz + last], |fall| {
        let [x, y, z] = fall.pos;
        visit(
            section_idx((x - ox) as usize, (y - oy) as usize, (z - oz) as usize),
            fall,
        );
    });
}

/// Stamp the fluid falls over a section's engine terrain, through the same
/// claims [`space_mask`] applies. Writes through the setter, so the section's
/// counters must already be current.
pub(crate) fn stamp_falls(caves: &CaveField, sp: SectionPos, section: &mut Section) {
    section_falls(caves, sp, |cell, fall| {
        let (x, y, z) = petramond_world::chunk::section_local(cell);
        if fall.admits(space_of(section.block_raw(x, y, z))) {
            section.set_fluid(x, y, z, Block::from_id(fall.fluid), fall.meta);
        }
    });
}

/// What the section's terrain leaves in each cell: [`terrain_cube`] with the
/// fluid falls stamped over it.
pub(crate) fn space_mask(
    surface: &SurfaceDensitySystem,
    caves: &CaveField,
    seed: u32,
    sp: SectionPos,
    biomes: &[u8],
    surf: &[i32],
) -> Arc<SpaceMask> {
    SPACES.get_or_insert(key(caves, seed, sp), || {
        let cube = terrain_cube(surface, caves, seed, sp, biomes, surf);
        let mut mask = SpaceMask {
            solid: [0; 64],
            fluid: [0; 64],
        };
        for (i, id) in cube.iter().enumerate() {
            mask.set(i, space_of(id));
        }
        section_falls(caves, sp, |cell, fall| {
            if fall.admits(mask.at(cell)) {
                mask.set(cell, mod_api::TerrainSpace::Fluid);
            }
        });
        Arc::new(mask)
    })
}

/// The mask when it is already known, without computing a section for it.
pub(crate) fn space_mask_if_memoized(
    caves: &CaveField,
    seed: u32,
    sp: SectionPos,
) -> Option<Arc<SpaceMask>> {
    SPACES.get(&key(caves, seed, sp))
}
