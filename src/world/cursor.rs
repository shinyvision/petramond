use crate::world::ServerWorld;
use std::cell::Cell;

use petramond_math::math::IVec3;
use petramond_world::block::{Aabb, Block};
use petramond_world::chunk::SectionPos;

pub struct SectionCursor<'w> {
    world: &'w ServerWorld,
    cells: petramond_world::world::SectionCursor<'w>,
    last_final: Cell<Option<(SectionPos, bool)>>,
}

impl ServerWorld {
    #[inline]
    pub fn cursor(&self) -> SectionCursor<'_> {
        SectionCursor {
            world: self,
            cells: self.data.cursor(),
            last_final: Cell::new(None),
        }
    }
}

impl<'w> SectionCursor<'w> {
    #[inline]
    pub fn physics_block(&self, c: IVec3) -> Block {
        self.cells.physics_block(c)
    }

    #[inline]
    pub fn fluid_cell(&self, c: IVec3) -> bool {
        self.cells.fluid_cell(c)
    }

    #[inline]
    pub fn fluid_meta(&self, c: IVec3) -> u8 {
        self.cells.fluid_meta(c)
    }

    #[inline]
    pub fn collision_boxes(&self, c: IVec3) -> &'static [Aabb] {
        self.cells.collision_boxes(c)
    }

    #[inline]
    pub fn collision_boxes_xyz(&self, x: i32, y: i32, z: i32) -> &'static [Aabb] {
        self.cells.collision_boxes_xyz(x, y, z)
    }

    #[inline]
    pub fn boxes_of(&self, c: IVec3, block: Block) -> &'static [Aabb] {
        self.cells.boxes_of(c, block)
    }

    #[inline]
    pub fn cell_final(&self, c: IVec3) -> bool {
        let Some(sp) = SectionPos::from_world(c.x, c.y, c.z) else {
            return self.world.physics_cell_final_at(c.x, c.y, c.z);
        };
        if let Some((last, verdict)) = self.last_final.get() {
            if last == sp {
                return verdict;
            }
        }
        let verdict = self.world.physics_cell_final_at(c.x, c.y, c.z);
        self.last_final.set(Some((sp, verdict)));
        verdict
    }
}
