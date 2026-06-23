//! Torch orientation at the world level: world-coordinate access to the
//! chunk-owned torch maps.
//!
//! A torch never ticks and — unlike a chest — is baked into the chunk mesh rather
//! than gathered per frame, so this is just thin world↔chunk wrappers for placement
//! and breaking. Mirrors [`world::chest`](super::chest) minus the GUI/gather paths.

use crate::mathh::IVec3;
use crate::torch::TorchPlacement;

use super::store::World;

impl World {
    /// How the torch at a world block position is mounted, or `Floor` if the cell
    /// holds no recorded torch (or its chunk is unloaded). Read by the raycast to
    /// build the torch-shaped selection outline.
    pub fn torch_placement(&self, pos: IVec3) -> TorchPlacement {
        match self.chunk_at_world(pos.x, pos.y, pos.z) {
            Some((c, lx, ly, lz)) => c.torch_placement(lx, ly, lz),
            None => TorchPlacement::default(),
        }
    }

    /// Record `placement` for a freshly placed torch block. No-op if the owning
    /// chunk is not loaded or `y` is out of range.
    pub fn insert_torch(&mut self, pos: IVec3, placement: TorchPlacement) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_torch(lx, ly, lz, placement);
        }
    }

    /// Forget a broken torch's orientation. No-op if the owning chunk is not loaded.
    pub fn take_torch(&mut self, pos: IVec3) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.take_torch(lx, ly, lz);
        }
    }
}
