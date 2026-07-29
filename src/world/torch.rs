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

    /// Whether `placement` has a usable support face for a torch placed at `pos`.
    /// Full opaque blocks accept the same floor/wall faces as before; partial blocks
    /// only accept complete flat faces.
    pub(crate) fn torch_supported_at(&self, pos: IVec3, placement: TorchPlacement) -> bool {
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
    pub(crate) fn wall_face_complete(&self, support: IVec3, normal: IVec3) -> bool {
        if normal.y != 0 || normal.x.abs() + normal.z.abs() != 1 {
            return false;
        }
        self.mount_face_complete(support, normal)
    }

    /// The face test behind [`block_supports_torch`](Self::block_supports_torch)
    /// and [`wall_face_complete`](Self::wall_face_complete): the support
    /// FAMILY answers whether its face toward the mount is complete
    /// (`ShapeSim::full_face` — no family knowledge here); a full-cube face
    /// additionally requires the opaque material, the historical mount rule.
    pub(crate) fn mount_face_complete(&self, support: IVec3, normal: IVec3) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::block_state::{SlabSplit, StairHalf, StairState};
    use crate::chunk::{Chunk, ChunkPos};
    use crate::facing::Facing;

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn stair_flat_back_supports_a_wall_torch() {
        let mut w = world();
        let stair = IVec3::new(8, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));

        let torch = stair - IVec3::new(1, 0, 0);
        assert!(
            w.torch_supported_at(torch, TorchPlacement::West),
            "the full-height back face of a stair should hold a wall torch"
        );
    }

    #[test]
    fn single_slab_side_does_not_support_a_wall_torch() {
        let mut w = world();
        let slab = IVec3::new(8, 64, 8);
        assert!(w.place_slab_layer(
            slab,
            Block::DirtSlab,
            crate::slab::SlabSlot {
                split: SlabSplit::Y,
                index: 0,
            }
        ));

        let torch = slab + IVec3::new(1, 0, 0);
        assert!(
            !w.torch_supported_at(torch, TorchPlacement::East),
            "a single slab side is not a complete wall face"
        );
    }

    #[test]
    fn stair_open_side_does_not_support_a_wall_torch() {
        let mut w = world();
        let stair = IVec3::new(8, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));

        let torch = stair + IVec3::new(1, 0, 0);
        assert!(
            !w.torch_supported_at(torch, TorchPlacement::East),
            "the open side of a stair is not a complete wall face"
        );
    }

    #[test]
    fn fence_post_top_supports_a_floor_torch_but_its_sides_hold_no_wall_torch() {
        let mut w = world();
        w.set_block_world(8, 64, 8, Block::OakFence);

        let floor_torch = IVec3::new(8, 65, 8);
        assert!(
            w.torch_supported_at(floor_torch, TorchPlacement::Floor),
            "a fence's post top should hold a floor torch"
        );

        let fence = IVec3::new(8, 64, 8);
        for (torch, placement) in [
            (fence + IVec3::new(1, 0, 0), TorchPlacement::East),
            (fence + IVec3::new(-1, 0, 0), TorchPlacement::West),
            (fence + IVec3::new(0, 0, 1), TorchPlacement::South),
            (fence + IVec3::new(0, 0, -1), TorchPlacement::North),
        ] {
            assert!(
                !w.torch_supported_at(torch, placement),
                "{placement:?} must not mount on a fence side"
            );
        }
    }

    #[test]
    fn full_slab_stacks_support_torches_like_full_blocks() {
        let mut w = world();
        let slab = IVec3::new(8, 64, 8);
        for (block, index) in [(Block::DirtSlab, 0), (Block::CobblestoneSlab, 1)] {
            assert!(w.place_slab_layer(
                slab,
                block,
                crate::slab::SlabSlot {
                    split: SlabSplit::Y,
                    index,
                }
            ));
        }

        for (torch, placement) in [
            (slab + IVec3::new(0, 1, 0), TorchPlacement::Floor),
            (slab + IVec3::new(1, 0, 0), TorchPlacement::East),
            (slab + IVec3::new(-1, 0, 0), TorchPlacement::West),
            (slab + IVec3::new(0, 0, 1), TorchPlacement::South),
            (slab + IVec3::new(0, 0, -1), TorchPlacement::North),
        ] {
            assert!(
                w.torch_supported_at(torch, placement),
                "{placement:?} should be supported by a full slab stack"
            );
        }
    }
}
