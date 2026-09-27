use crate::mathh::IVec3;
use crate::torch::TorchPlacement;

use super::data::WorldData;

impl WorldData {
    pub fn torch_placement(&self, pos: IVec3) -> TorchPlacement {
        match self.chunk_at_world(pos.x, pos.y, pos.z) {
            Some((c, lx, ly, lz)) => c.torch_placement(lx, ly, lz),
            None => TorchPlacement::default(),
        }
    }

    pub fn insert_torch(&mut self, pos: IVec3, placement: TorchPlacement) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_torch(lx, ly, lz, placement);
        }
    }

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

    pub fn wall_face_complete(&self, support: IVec3, normal: IVec3) -> bool {
        if normal.y != 0 || normal.x.abs() + normal.z.abs() != 1 {
            return false;
        }
        self.mount_face_complete(support, normal)
    }

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

#[cfg(test)]
mod tests;
