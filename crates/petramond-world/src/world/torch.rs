//! Torch orientation at the world level: world-coordinate access to the
//! chunk-owned torch maps.
//!
//! A torch never ticks and — unlike a chest — is baked into the chunk mesh rather
//! than gathered per frame, so this is just thin world↔chunk wrappers for placement
//! and breaking. Mirrors `world::chest` minus the GUI/gather paths.

use crate::mathh::IVec3;
use crate::torch::TorchPlacement;

use super::data::WorldData;

impl WorldData {
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

    /// Whether `placement` has a usable support face for a torch placed at `pos`.
    /// Full opaque blocks accept the same floor/wall faces as before; partial blocks
    /// only accept complete flat faces.
    pub fn torch_supported_at(&self, pos: IVec3, placement: TorchPlacement) -> bool {
        self.block_supports_torch(
            placement.support_cell(pos),
            placement.support_normal(),
            placement,
        )
    }

    fn block_supports_torch(
        &self,
        support: IVec3,
        normal: IVec3,
        placement: TorchPlacement,
    ) -> bool {
        if support_kind(normal, placement).is_none() {
            return false;
        }
        self.mount_face_complete(support, normal)
    }

    /// Whether the VERTICAL face of the block at `support` whose outward unit
    /// normal is `normal` is a complete flat 1×1 face able to hold a wall-mounted
    /// block — the shared wall-support rule behind the wall torch and the ladder.
    /// Full opaque blocks always qualify; partial blocks (stairs, slabs) only
    /// through a genuinely complete face. Non-horizontal normals never qualify.
    pub fn wall_face_complete(&self, support: IVec3, normal: IVec3) -> bool {
        if normal.y != 0 || normal.x.abs() + normal.z.abs() != 1 {
            return false;
        }
        self.mount_face_complete(support, normal)
    }

    /// The face test behind `block_supports_torch`
    /// and [`wall_face_complete`](Self::wall_face_complete): the support
    /// FAMILY answers whether its face toward the mount is complete
    /// (`ShapeSim::full_face` — no family knowledge here); a full-cube face
    /// additionally requires the opaque material, the historical mount rule.
    pub fn mount_face_complete(&self, support: IVec3, normal: IVec3) -> bool {
        match crate::block::full_face_at(self, support, normal) {
            Some(crate::block::FullFace::Cube) => self
                .physics_block(support.x, support.y, support.z)
                .is_opaque(),
            Some(crate::block::FullFace::Shaped) => true,
            None => false,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SupportKind {
    Floor,
    Wall,
}

fn support_kind(normal: IVec3, placement: TorchPlacement) -> Option<SupportKind> {
    match (normal.x, normal.y, normal.z) {
        (0, 1, 0) if placement == TorchPlacement::Floor => Some(SupportKind::Floor),
        (_, 0, _) if placement.is_wall() && normal.x.abs() + normal.z.abs() == 1 => {
            Some(SupportKind::Wall)
        }
        _ => None,
    }
}

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    #[allow(unused_imports)]
    pub use super::*;
    pub use crate::mathh::IVec3;
    pub use crate::torch::TorchPlacement;
}
