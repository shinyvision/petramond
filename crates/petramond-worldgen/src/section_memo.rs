//! A section's terrain before features — filled and carved — memoized
//! process-wide, because two consumers need exactly it: section generation,
//! and the positional terrain queries a pack asks while placing content that
//! spans sections. A pack probing a cell's floor shortly before its sections
//! stream, or dressing a section just after, otherwise paid the fill and the
//! carve twice. Solidity is kept separately and longer than the blocks: it is
//! sixteen times smaller and is what the queries read.

use std::sync::{Arc, LazyLock};

use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;
use petramond_world::section::{BlockCube, Section};

use crate::density::surface::SurfaceDensitySystem;
use crate::memo::SharedMemo;
use crate::noise::cave_field::CaveField;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    seed: u32,
    tables: [usize; 2],
    pos: [i32; 3],
}

/// One bit per section-local cell index: neither air nor water.
pub(crate) type SolidMask = [u64; 64];

static CUBES: LazyLock<SharedMemo<Key, BlockCube>> = LazyLock::new(|| SharedMemo::new(8192));
static SOLID: LazyLock<SharedMemo<Key, Arc<SolidMask>>> = LazyLock::new(|| SharedMemo::new(16384));

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

/// Which cells of the section are solid terrain, from [`terrain_cube`].
pub(crate) fn solid_mask(
    surface: &SurfaceDensitySystem,
    caves: &CaveField,
    seed: u32,
    sp: SectionPos,
    biomes: &[u8],
    surf: &[i32],
) -> Arc<SolidMask> {
    SOLID.get_or_insert(key(caves, seed, sp), || {
        let cube = terrain_cube(surface, caves, seed, sp, biomes, surf);
        let mut mask = [0u64; 64];
        for (i, id) in cube.iter().enumerate() {
            if Block::from_id(id).is_solid() {
                mask[i / 64] |= 1 << (i % 64);
            }
        }
        Arc::new(mask)
    })
}

/// The mask when it is already known, without computing a section for it.
pub(crate) fn solid_mask_if_memoized(
    caves: &CaveField,
    seed: u32,
    sp: SectionPos,
) -> Option<Arc<SolidMask>> {
    SOLID.get(&key(caves, seed, sp))
}
