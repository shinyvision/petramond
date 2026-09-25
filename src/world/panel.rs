//! Hinged panels — doors and trapdoors — share one presentation seam.
//!
//! Their geometry has nothing in common, but their ANIMATION does: a swing
//! eases toward the one open/closed bit the placed panel carries. That read is
//! here so the client's swing pump asks a single question of a cell instead of
//! one per kind of panel.

use petramond_math::math::IVec3;

use super::store::World;

impl World {
    /// Whether the hinged panel at `pos` stands open, or `None` when the cell
    /// holds no panel (or is unloaded).
    #[inline]
    pub fn panel_open_at(&self, pos: IVec3) -> Option<bool> {
        if let Some(door) = self.door_state_at(pos.x, pos.y, pos.z) {
            return Some(door.open);
        }
        Some(self.trapdoor_state_at(pos.x, pos.y, pos.z)?.open)
    }
}
