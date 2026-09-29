use std::cell::Cell;

use crate::block::{Aabb, Block, ShapeNeighborhood, ShapeRenderBox, ShapeState};
use crate::chunk::{section_idx, ChunkPos};
use crate::mathh::IVec3;
use crate::section::Section;

use super::data::WorldData;
use super::section_map::ColumnSlots;

/// Reads through the world with the last-touched COLUMN cached: a neighbour above or below
/// is a slot index away, a neighbour beside is one column probe.
pub struct SectionCursor<'w> {
    data: &'w WorldData,
    last: Cell<Option<(ChunkPos, &'w ColumnSlots)>>,
}

impl WorldData {
    #[inline]
    pub fn cursor(&self) -> SectionCursor<'_> {
        SectionCursor {
            data: self,
            last: Cell::new(None),
        }
    }
}

impl<'w> SectionCursor<'w> {
    #[inline]
    pub fn data(&self) -> &'w WorldData {
        self.data
    }

    #[inline]
    pub fn section_at(&self, c: IVec3) -> Option<(&'w Section, usize, usize, usize)> {
        let (sp, lx, ly, lz) = WorldData::split_world(c.x, c.y, c.z)?;
        let cp = sp.chunk_pos();
        let column = match self.last.get() {
            Some((last, column)) if last == cp => column,
            _ => {
                let column = self.data.sections.column(cp)?;
                self.last.set(Some((cp, column)));
                column
            }
        };
        let section = column.at(sp.cy)?;
        Some((&**section, lx, ly, lz))
    }

    #[inline]
    pub fn chunk_block(&self, c: IVec3) -> u16 {
        match self.section_at(c) {
            Some((s, lx, ly, lz)) => s.block_raw(lx, ly, lz),
            None => 0,
        }
    }

    #[inline]
    pub fn block_if_loaded(&self, c: IVec3) -> Option<Block> {
        let (s, lx, ly, lz) = self.section_at(c)?;
        Some(s.block(lx, ly, lz))
    }

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

    #[inline]
    pub fn blocks_movement(&self, c: IVec3) -> bool {
        self.physics_block(c).blocks_movement()
    }

    #[inline]
    pub fn fluid_cell(&self, c: IVec3) -> bool {
        self.physics_block(c).fluid().is_some()
    }

    #[inline]
    pub fn fluid_meta(&self, c: IVec3) -> u8 {
        match self.section_at(c) {
            Some((s, lx, ly, lz)) => s.fluid_meta(lx, ly, lz),
            None => 0,
        }
    }

    #[inline]
    pub fn collision_boxes(&self, c: IVec3) -> &'static [Aabb] {
        self.boxes_of(c, self.physics_block(c))
    }

    #[inline]
    pub fn boxes_of(&self, c: IVec3, block: Block) -> &'static [Aabb] {
        if let Some(boxes) = block.static_collision_boxes() {
            return boxes;
        }
        let k = block.shape_kind_def();
        k.sim.collision_boxes(&k.params, self, c, block)
    }

    #[inline]
    pub fn collision_boxes_xyz(&self, x: i32, y: i32, z: i32) -> &'static [Aabb] {
        self.collision_boxes(IVec3::new(x, y, z))
    }
}

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
    use crate::chunk::{SectionPos, SECTION_SIZE};

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
                    assert_eq!(
                        cur.collision_boxes(c),
                        data.collision_boxes_at(x, y, z),
                        "{c:?}"
                    );
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
