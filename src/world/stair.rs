//! Directional stairs at the world level: position-aware facing lookup and placement.

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::block_state::StairState;

use super::store::World;

impl World {
    /// Place a single-cell stair and record its facing before relighting/remeshing.
    /// Assumes the caller already gated replaceability and entity overlap.
    pub fn place_stair(&mut self, pos: IVec3, block: Block, state: StairState) -> bool {
        if !petramond_world::stair::is_stair(block) || !self.materialize_section_at(pos) {
            return false;
        }
        let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, block);
        section.set_stair_state(lx, ly, lz, state);
        section.modified = true;
        self.refresh_region(&[pos]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use petramond_world::block_state::StairHalf;
    use petramond_world::chunk::ChunkPos;

    #[test]
    fn placing_a_stair_raises_the_column_surface_for_skylight() {
        let mut world = World::new(0, 0);
        let p = IVec3::new(8, 8, 8);

        assert!(world.place_stair(
            p,
            Block::OakStairs,
            StairState::new(petramond_math::facing::Facing::East, StairHalf::Bottom)
        ));

        let column = world.columns.get(&ChunkPos::new(0, 0)).unwrap();
        assert_eq!(
            column.surface_y(8, 8),
            8,
            "a placed stair roof must become sky cover for the column heightmap"
        );
    }
}
