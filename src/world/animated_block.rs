//! Animated blocks — every row that draws an animated block model (a chest's
//! lid, a door's swing, a trapdoor's panel, a pack's barrel) — share ONE
//! gather and ONE pose read.
//!
//! The model, its variants and its hinges are data
//! ([`petramond_world::animated_model`]); how a cell poses it is its shape
//! family's answer ([`Block::animated_pose`]). Nothing here knows which kind
//! of block it is gathering, so a new animated block needs no edit here.
//!
//! An animated cell is found through the block-entity section index, which
//! admits a cell by its stored state: every animated row stores one (its
//! placement front, or its shape's own state).

#[cfg(test)]
use crate::world::ServerWorld;
use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::animated_model::{AnimatedModelDef, AnimatedPose};
use petramond_world::block::{Block, ShapeNeighborhood};
use petramond_world::light::BlockLight6;

/// One animated block to draw this frame: where, what, how it is posed by its
/// own state, and the light at its cell. The client eases the open fraction
/// on top (see `petramond_client`'s block animations).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AnimatedBlock {
    pub pos: IVec3,
    pub block: Block,
    pub pose: AnimatedPose,
    pub skylight: u8,
    pub blocklight: BlockLight6,
}

impl<S: WorldSide> World<S> {
    /// Gather every loaded animated block into `out` (cleared first). Visits
    /// only the block-entity section index, not every loaded section.
    pub fn collect_animated_blocks(&self, out: &mut Vec<AnimatedBlock>) {
        out.clear();
        for sp in &self.data.block_entity_sections {
            let Some(section) = self.data.sections.get(sp) else {
                continue;
            };
            let (ox, oy, oz) = section.origin_world();
            for (&key, &state) in section.cell_states() {
                let (lx, ly, lz) = petramond_world::chunk::section_local(key as usize);
                let block = section.block(lx, ly, lz);
                let Some((_, pose)) = block.animated_pose(state) else {
                    continue;
                };
                let pos = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                out.push(AnimatedBlock {
                    pos,
                    block,
                    pose,
                    skylight: self.data.skylight6_at_world(pos.x, pos.y, pos.z),
                    blocklight: BlockLight6::from_x2(
                        self.data.blocklight_rgb_at_world(pos.x, pos.y, pos.z),
                    ),
                });
            }
        }
    }

    /// The animated model at `pos` and how the cell poses it, or `None` when
    /// the cell draws none itself (no animated row, a compound member its
    /// anchor draws, or an unloaded cell).
    pub fn animated_pose_at(
        &self,
        pos: IVec3,
    ) -> Option<(&'static AnimatedModelDef, AnimatedPose)> {
        let block = ShapeNeighborhood::block(&self.data, pos);
        block.animated_pose(ShapeNeighborhood::shape_state(&self.data, pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::facing::Facing;
    use petramond_world::block::CellCodec;
    use petramond_world::chunk::{Chunk, ChunkPos};
    use petramond_world::trapdoor::TrapdoorState;
    use petramond_world::world::placement::PlacementPlan;

    fn world() -> ServerWorld {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn one_gather_finds_chests_doors_and_trapdoors_once_each() {
        let mut w = world();
        let chest = IVec3::new(2, 64, 2);
        w.set_block_world(chest.x, chest.y, chest.z, Block::Chest);
        w.insert_chest(chest, Facing::East);
        let door = IVec3::new(5, 64, 5);
        w.set_block_world(door.x, door.y - 1, door.z, Block::Stone);
        assert!(w.place_door(door, Block::OakDoor, Facing::South));
        let hatch = IVec3::new(8, 64, 8);
        let state = TrapdoorState {
            facing: Facing::West,
            open: true,
            top: true,
        };
        assert!(w.commit_placement(
            &PlacementPlan::single(hatch, Block::OakTrapdoor, state.to_cell()),
            true
        ));

        let mut rows = Vec::new();
        w.collect_animated_blocks(&mut rows);
        rows.sort_by_key(|r| r.pos.x);
        let got: Vec<_> = rows.iter().map(|r| (r.pos, r.block, r.pose)).collect();
        let pose = |facing, variant, open| AnimatedPose {
            facing,
            variant,
            open,
        };
        assert_eq!(
            got,
            vec![
                (chest, Block::Chest, pose(Facing::East, 0, false)),
                // Once, from the lower half: the upper is drawn with it.
                (door, Block::OakDoor, pose(Facing::South, 0, false)),
                (hatch, Block::OakTrapdoor, pose(Facing::West, 1, true)),
            ]
        );
    }

    #[test]
    fn the_pose_read_follows_a_toggle() {
        let mut w = world();
        let door = IVec3::new(5, 64, 5);
        w.set_block_world(door.x, door.y - 1, door.z, Block::Stone);
        assert!(w.place_door(door, Block::OakDoor, Facing::North));
        assert_eq!(w.animated_pose_at(door).map(|(_, p)| p.open), Some(false));
        w.toggle_door(door + IVec3::Y);
        assert_eq!(w.animated_pose_at(door).map(|(_, p)| p.open), Some(true));
        assert!(
            w.animated_pose_at(door + IVec3::Y).is_none(),
            "the upper half draws nothing itself"
        );
        assert!(w.animated_pose_at(IVec3::new(0, 64, 0)).is_none());
    }
}
