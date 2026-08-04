//! A read cursor over the section grid, for probe-bound walks.
//!
//! Every world-coordinate read (`physics_block`, `collision_boxes_at`,
//! `water_cell_at`, `physics_cell_final_at`) resolves its section through one
//! `FxHashMap<SectionPos, Arc<Section>>` lookup plus an `Arc` deref — two
//! likely cache misses per CELL. Navigation is the tick's hot loop and walks
//! contiguous cells: a confinement fill, an A* expansion and a body sweep all
//! ask about neighbours of the cell they just asked about, so the section is
//! overwhelmingly the one they resolved last.
//!
//! [`SectionCursor`] remembers the last section (and the last streaming
//! verdict) and answers from it when the next cell falls inside. It borrows
//! the world immutably for its whole life, so the borrow checker — not a
//! hand-maintained invalidation hook — is what proves the cached reference
//! still points at the live section.

use std::cell::Cell;

use crate::block::{Aabb, Block};
use crate::chunk::{self, SectionPos, SECTION_SIZE};
use crate::mathh::IVec3;
use crate::section::Section;

use super::store::World;

pub struct SectionCursor<'w> {
    world: &'w World,
    /// The last section resolved, `None` until the first hit. A miss (absent
    /// section) is deliberately NOT cached: absent sections fall through to
    /// the generated-summary path, which the cursor does not shortcut.
    last: Cell<Option<(SectionPos, &'w Section)>>,
    /// The last `physics_cell_final_at` verdict, which is a per-SECTION fact.
    last_final: Cell<Option<(SectionPos, bool)>>,
}

impl World {
    /// A read cursor over this world (see [`SectionCursor`]). Free to make;
    /// make one per probe-bound walk and share it between that walk's probes.
    #[inline]
    pub fn cursor(&self) -> SectionCursor<'_> {
        SectionCursor {
            world: self,
            last: Cell::new(None),
            last_final: Cell::new(None),
        }
    }
}

impl<'w> SectionCursor<'w> {
    /// The loaded section owning `(wx, wy, wz)` plus its section-local coords.
    #[inline]
    fn section_at(&self, wx: i32, wy: i32, wz: i32) -> Option<(&'w Section, usize, usize, usize)> {
        let sp = SectionPos::from_world(wx, wy, wz)?;
        let local = (
            chunk::lx(wx),
            wy.rem_euclid(SECTION_SIZE as i32) as usize,
            chunk::lz(wz),
        );
        if let Some((last_pos, section)) = self.last.get() {
            if last_pos == sp {
                return Some((section, local.0, local.1, local.2));
            }
        }
        let section = self.world.section_ref(sp)?;
        self.last.set(Some((sp, section)));
        Some((section, local.0, local.1, local.2))
    }

    /// Mirror of [`World::physics_block`].
    #[inline]
    pub fn physics_block(&self, c: IVec3) -> Block {
        match self.section_at(c.x, c.y, c.z) {
            Some((s, lx, ly, lz)) => s.block(lx, ly, lz),
            None => self.world.physics_block(c.x, c.y, c.z),
        }
    }

    /// Mirror of [`World::water_cell_at`].
    #[inline]
    pub fn water_cell(&self, c: IVec3) -> bool {
        self.physics_block(c) == Block::Water
    }

    /// Mirror of [`World::collision_boxes_at`], taking the dense per-id table
    /// first exactly like it does.
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
        k.sim.collision_boxes(&k.params, self.world, c, block)
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
