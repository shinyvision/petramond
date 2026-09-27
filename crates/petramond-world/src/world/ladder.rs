use crate::block::Block;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::data::WorldData;

impl WorldData {
    pub fn ladder_supported_at(&self, pos: IVec3, facing: Facing) -> bool {
        let dir = facing.dir();
        self.wall_face_complete(crate::ladder::support_cell(pos, facing), dir)
    }

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Climb {
    Panel(Facing),
    Free,
}

#[cfg(test)]
mod tests;
