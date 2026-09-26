//! The engine's read cursor for probe-bound walks: the world crate's
//! [`petramond_world::world::SectionCursor`] (the cached last-section resolve
//! every voxel read goes through) plus the one per-section fact only the
//! engine knows — the streaming-finality verdict navigation gates on.
//!
//! Navigation is the tick's hot loop and walks contiguous cells: a confinement
//! fill, an A* expansion and a body sweep all ask about neighbours of the cell
//! they just asked about. The cursor borrows the world immutably for its whole
//! life, so the borrow checker — not a hand-maintained invalidation hook — is
//! what proves the cached references still point at live sections.

use crate::world::ServerWorld;
use std::cell::Cell;

use petramond_math::math::IVec3;
use petramond_world::block::{Aabb, Block};
use petramond_world::chunk::SectionPos;


pub struct SectionCursor<'w> {
    world: &'w ServerWorld,
    cells: petramond_world::world::SectionCursor<'w>,
    /// The last `physics_cell_final_at` verdict, which is a per-SECTION fact.
    last_final: Cell<Option<(SectionPos, bool)>>,
}

impl ServerWorld {
    /// A read cursor over this world (see [`SectionCursor`]). Free to make;
    /// make one per probe-bound walk and share it between that walk's probes.
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
    /// Mirror of `World::physics_block`.
    #[inline]
    pub fn physics_block(&self, c: IVec3) -> Block {
        self.cells.physics_block(c)
    }

    /// Mirror of `World::fluid_cell_at`.
    #[inline]
    pub fn fluid_cell(&self, c: IVec3) -> bool {
        self.cells.fluid_cell(c)
    }

    /// Mirror of `World::fluid_meta_world`.
    #[inline]
    pub fn fluid_meta(&self, c: IVec3) -> u8 {
        self.cells.fluid_meta(c)
    }

    /// Mirror of `World::collision_boxes_at`.
    #[inline]
    pub fn collision_boxes(&self, c: IVec3) -> &'static [Aabb] {
        self.cells.collision_boxes(c)
    }

    /// [`collision_boxes`](Self::collision_boxes) in the `(x, y, z)` shape
    /// the swept-AABB resolver's box source takes.
    #[inline]
    pub fn collision_boxes_xyz(&self, x: i32, y: i32, z: i32) -> &'static [Aabb] {
        self.cells.collision_boxes_xyz(x, y, z)
    }

    /// The boxes of a block already read at `c` — so a probe that needs both
    /// the block and its boxes reads the cell once.
    #[inline]
    pub fn boxes_of(&self, c: IVec3, block: Block) -> &'static [Aabb] {
        self.cells.boxes_of(c, block)
    }

    /// Mirror of [`World::physics_cell_final_at`] — a per-section fact, so it
    /// is cached per section rather than per cell.
    #[inline]
    pub fn cell_final(&self, c: IVec3) -> bool {
        let Some(sp) = SectionPos::from_world(c.x, c.y, c.z) else {
            // Outside the world reads air forever; defer to the world's own
            // answer rather than duplicating the rule.
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
