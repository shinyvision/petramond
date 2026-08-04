//! Ladder state at the world level: the wall-support rule and the climbable
//! query the player physics samples.
//!
//! The facing needs no world-level accessor at all: which wall a ladder hangs
//! on is block IDENTITY (one row per facing, `Block::panel_facing` — see
//! `crate::ladder`), so persistence, replication, and the break sweep are the
//! ordinary block-id lanes and callers read the facing off the block they
//! already fetched. This module only adds the ladder-specific support rule and
//! the physics probe. Mirrors [`world::torch`](super::torch).

use crate::block::Block;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::data::WorldData;

impl WorldData {
    /// Whether a ladder facing `facing` at `pos` has a usable wall behind it:
    /// the support cell's face toward the ladder must be a complete vertical
    /// face (opaque block, stair back, full slab side — the same rule as the
    /// wall torch). Gates placement, the predicted ghost, and the FRAGILE
    /// support re-check, so all three agree by construction.
    pub fn ladder_supported_at(&self, pos: IVec3, facing: Facing) -> bool {
        let dir = facing.dir();
        self.wall_face_complete(crate::ladder::support_cell(pos, facing), dir)
    }

    /// The climbable cell sample the player physics probes each sub-step: how
    /// the block at the cell is climbed, or `None` when the cell holds no
    /// climbable block (or its section is unloaded). One section lookup and a
    /// dense flag read gate it — no `def()` table walk until the cell actually
    /// climbs; the grip then comes off the row of the id already fetched, so no
    /// second per-cell map traversal exists at all.
    pub fn climb_at(&self, x: i32, y: i32, z: i32) -> Option<Climb> {
        let (s, lx, ly, lz) = self.chunk_at_world(x, y, z)?;
        let block = Block::from_id(s.block_raw(lx, ly, lz));
        block
            .is_climbable()
            .then(|| match block.declared_panel_facing() {
                Some(facing) => Climb::Panel(facing),
                None => Climb::Free,
            })
    }
}

/// How a climbable cell is ascended — ROW DATA, not a block-kind check.
///
/// A wall panel declares a `panel_facing` (only the `ladder` shape may), so
/// pressing into the wall it hangs on climbs it. A row without one hangs free
/// in the air — the exploration pack's vine curtains — where there is no
/// direction to press, so the jump button is the only way up. Deriving this
/// from the DECLARED facing rather than [`Block::panel_facing`] is the whole
/// point: that accessor defaults to North, which would make walking south
/// climb a vine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Climb {
    /// Press toward this facing's wall, or hold jump.
    Panel(Facing),
    /// Hold jump.
    Free,
}

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    pub use crate::block::Block;
    pub use crate::facing::Facing;
    pub use crate::mathh::IVec3;
    #[allow(unused_imports)]
    pub use super::*;
}
