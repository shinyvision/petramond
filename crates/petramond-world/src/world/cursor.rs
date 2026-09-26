//! A read cursor over the section grid, for walks over neighbouring cells.
//!
//! Every world-coordinate read on [`WorldData`] (`physics_block`,
//! `collision_boxes_at`, `fluid_meta_world`, the shape seam) resolves its
//! section through one `FxHashMap<SectionPos, Arc<Section>>` lookup plus an
//! `Arc` deref — two likely cache misses per CELL. The hot walks — a body's
//! collision sweep, a fluid column probe, a surface scan, a shape facet
//! asking about its neighbours — all ask about cells next to the one they just
//! asked about, so the section is overwhelmingly the one resolved last.
//!
//! [`SectionCursor`] remembers the last section and answers from it when the
//! next cell falls inside. It borrows the world immutably for its whole life,
//! so the borrow checker — not a hand-maintained invalidation hook — is what
//! proves the cached reference still points at the live section. Every read
//! mirrors its `WorldData` twin exactly (same fallbacks for unloaded and
//! out-of-range cells); only the lookup is cheaper.

use std::cell::Cell;

use crate::block::{Aabb, Block, ShapeNeighborhood, ShapeRenderBox, ShapeState};
use crate::chunk::{section_idx, SectionPos};
use crate::mathh::IVec3;
use crate::section::Section;

use super::data::WorldData;

pub struct SectionCursor<'w> {
    data: &'w WorldData,
    /// The last section resolved, `None` until the first hit. A miss (absent
    /// section) is deliberately NOT cached: absent sections fall through to
    /// the generated-summary path, which the cursor does not shortcut.
    last: Cell<Option<(SectionPos, &'w Section)>>,
}

impl WorldData {
    /// A read cursor over this world (see [`SectionCursor`]). Free to make;
    /// make one per walk and share it between that walk's probes.
    #[inline]
    pub fn cursor(&self) -> SectionCursor<'_> {
        SectionCursor {
            data: self,
            last: Cell::new(None),
        }
    }
}

impl<'w> SectionCursor<'w> {
    /// The world this cursor reads.
    #[inline]
    pub fn data(&self) -> &'w WorldData {
        self.data
    }

    /// The loaded section owning world cell `c` plus its section-local
    /// coords — [`WorldData::chunk_at_world`] through the cache.
    #[inline]
    pub fn section_at(&self, c: IVec3) -> Option<(&'w Section, usize, usize, usize)> {
        let (sp, lx, ly, lz) = WorldData::split_world(c.x, c.y, c.z)?;
        if let Some((last_pos, section)) = self.last.get() {
            if last_pos == sp {
                return Some((section, lx, ly, lz));
            }
        }
        let section = self.data.section_ref(sp)?;
        self.last.set(Some((sp, section)));
        Some((section, lx, ly, lz))
    }

    /// Mirror of [`WorldData::chunk_block`].
    #[inline]
    pub fn chunk_block(&self, c: IVec3) -> u16 {
        match self.section_at(c) {
            Some((s, lx, ly, lz)) => s.block_raw(lx, ly, lz),
            None => 0,
        }
    }

    /// Mirror of [`WorldData::block_if_loaded`].
    #[inline]
    pub fn block_if_loaded(&self, c: IVec3) -> Option<Block> {
        let (s, lx, ly, lz) = self.section_at(c)?;
        Some(s.block(lx, ly, lz))
    }

    /// Mirror of [`WorldData::physics_block`]: the border wall first, then the
    /// loaded cell, then the absent section's generated summary.
    #[inline]
    pub fn physics_block(&self, c: IVec3) -> Block {
        if !crate::border::contains_column(c.x, c.z) {
            return Block::Stone;
        }
        match self.section_at(c) {
            Some((s, lx, ly, lz)) => s.block(lx, ly, lz),
            None => self.data.physics_block(c.x, c.y, c.z),
        }
    }

    /// Mirror of [`WorldData::blocks_movement_at`].
    #[inline]
    pub fn blocks_movement(&self, c: IVec3) -> bool {
        self.physics_block(c).blocks_movement()
    }

    /// Mirror of [`WorldData::fluid_cell_at`].
    #[inline]
    pub fn fluid_cell(&self, c: IVec3) -> bool {
        self.physics_block(c).fluid().is_some()
    }

    /// Mirror of [`WorldData::fluid_meta_world`].
    #[inline]
    pub fn fluid_meta(&self, c: IVec3) -> u8 {
        match self.section_at(c) {
            Some((s, lx, ly, lz)) => s.fluid_meta(lx, ly, lz),
            None => 0,
        }
    }

    /// Mirror of [`WorldData::collision_boxes_at`], taking the dense per-id
    /// table first exactly like it does; a stateful shape resolves its boxes
    /// with THIS cursor as its neighbourhood, so its neighbour reads hit the
    /// cache too.
    #[inline]
    pub fn collision_boxes(&self, c: IVec3) -> &'static [Aabb] {
        self.boxes_of(c, self.physics_block(c))
    }

    /// The boxes of a block already read at `c` — so a probe that needs both
    /// the block and its boxes reads the cell once.
    #[inline]
    pub fn boxes_of(&self, c: IVec3, block: Block) -> &'static [Aabb] {
        if let Some(boxes) = block.static_collision_boxes() {
            return boxes;
        }
        let k = block.shape_kind_def();
        k.sim.collision_boxes(&k.params, self, c, block)
    }

    /// [`collision_boxes`](Self::collision_boxes) in the `(x, y, z)` shape
    /// the swept-AABB resolver's box source takes.
    #[inline]
    pub fn collision_boxes_xyz(&self, x: i32, y: i32, z: i32) -> &'static [Aabb] {
        self.collision_boxes(IVec3::new(x, y, z))
    }
}

/// The shape seam through the cache — the exact reads of `WorldData`'s own
/// [`ShapeNeighborhood`] impl.
impl ShapeNeighborhood for SectionCursor<'_> {
    fn block(&self, pos: IVec3) -> Block {
        self.physics_block(pos)
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        match self.section_at(pos) {
            Some((s, lx, ly, lz)) => s.cell_state(lx, ly, lz),
            None => ShapeState::NONE,
        }
    }

    fn baked(&self, pos: IVec3) -> Option<&[ShapeRenderBox]> {
        let (s, lx, ly, lz) = self.section_at(pos)?;
        s.shape_render_boxes(section_idx(lx, ly, lz) as u16)
    }

    fn baked_collision(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.data.custom_shape_boxes(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::SECTION_SIZE;

    fn world_with_two_sections() -> WorldData {
        let mut data = WorldData::new(0, 4);
        for (pos, block) in [
            (SectionPos::new(0, 0, 0), Block::Stone),
            (SectionPos::new(1, 0, 0), Block::Glass),
        ] {
            let mut section = Section::new(pos.cx, pos.cy, pos.cz);
            for i in 0..SECTION_SIZE {
                section.set_block(i, i, 3, block);
            }
            data.sections.insert(pos, std::sync::Arc::new(section));
        }
        data
    }

    /// Every cursor read must answer exactly what the uncached `WorldData`
    /// read answers — across a section seam, into an absent section, and past
    /// the vertical range — whatever section the cursor last cached.
    #[test]
    fn cursor_reads_mirror_the_uncached_world_reads() {
        let data = world_with_two_sections();
        let cur = data.cursor();
        for y in -20..40 {
            for x in -4..36 {
                for z in [2, 3, 4] {
                    let c = IVec3::new(x, y, z);
                    assert_eq!(cur.chunk_block(c), data.chunk_block(x, y, z), "{c:?}");
                    assert_eq!(cur.physics_block(c), data.physics_block(x, y, z), "{c:?}");
                    assert_eq!(cur.fluid_meta(c), data.fluid_meta_world(x, y, z), "{c:?}");
                    assert_eq!(cur.collision_boxes(c), data.collision_boxes_at(x, y, z), "{c:?}");
                    assert_eq!(
                        cur.shape_state(c),
                        ShapeNeighborhood::shape_state(&data, c),
                        "{c:?}"
                    );
                }
            }
        }
    }
}
