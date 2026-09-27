#[cfg(test)]
use crate::world::ServerWorld;
use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::trapdoor::TrapdoorState;

impl<S: WorldSide> World<S> {
    #[inline]
    pub fn trapdoor_state_at(&self, wx: i32, wy: i32, wz: i32) -> Option<TrapdoorState> {
        let (c, lx, ly, lz) = self.data.chunk_at_world(wx, wy, wz)?;
        c.trapdoor_state(lx, ly, lz)
    }

    pub fn toggle_trapdoor(&mut self, pos: IVec3) -> Option<bool> {
        let mut state = self.trapdoor_state_at(pos.x, pos.y, pos.z)?;
        state.open = !state.open;
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.set_trapdoor_state(lx, ly, lz, state);
        }
        self.record_block_delta(pos.x, pos.y, pos.z);
        self.push_nav_change(pos);
        Some(state.open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::facing::Facing;
    use petramond_world::block::{Block, CellCodec, CellView};
    use petramond_world::chunk::{Chunk, ChunkPos};
    use petramond_world::world::placement::PlacementPlan;

    const TRAPDOOR: Block = Block::OakTrapdoor;

    fn world_with_a_trapdoor(top: bool) -> (ServerWorld, IVec3) {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let pos = IVec3::new(5, 64, 5);
        let state = TrapdoorState {
            facing: Facing::South,
            open: false,
            top,
        };
        assert!(w.commit_placement(&PlacementPlan::single(pos, TRAPDOOR, state.to_cell()), true));
        (w, pos)
    }

    #[test]
    fn a_placed_trapdoor_reaches_the_render_gather() {
        for top in [false, true] {
            let (w, pos) = world_with_a_trapdoor(top);
            let mut rows = Vec::new();
            w.collect_animated_blocks(&mut rows);
            assert_eq!(rows.len(), 1, "top={top}: one gathered panel");
            assert_eq!(rows[0].pos, pos);
            assert_eq!(rows[0].pose.variant, u8::from(top));
            assert!(!rows[0].pose.open);
        }
    }

    fn clicked(normal: IVec3, spot_y: f32, player_facing: Facing) -> TrapdoorState {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let hit = IVec3::new(5, 64, 5);
        w.set_block_world(hit.x, hit.y, hit.z, Block::Stone);
        let inputs = petramond_world::world::placement::PlaceInputs {
            hit,
            normal,
            spot: [0.5, spot_y, 0.5],
            place_pos: hit + normal,
            replacing_in_place: false,
            player_facing,
            held_rotation: Default::default(),
            held: None,
        };
        let plan = w
            .placement_plan(TRAPDOOR, &inputs, &mut |_, _| false)
            .expect("a clear cell plans");
        TrapdoorState::from_cell(plan.writes[0].state)
    }

    #[test]
    fn a_wall_click_hinges_on_that_wall_in_the_half_it_landed_in() {
        for (normal, hinge) in [
            (IVec3::X, Facing::West),
            (IVec3::NEG_X, Facing::East),
            (IVec3::Z, Facing::North),
            (IVec3::NEG_Z, Facing::South),
        ] {
            for (spot_y, top) in [(0.2, false), (0.8, true)] {
                let state = clicked(normal, spot_y, Facing::South);
                assert_eq!(state.facing, hinge, "{normal:?} hinges on the clicked wall");
                assert_eq!(state.top, top, "{normal:?} @ {spot_y}: which half");
            }
        }
    }

    #[test]
    fn a_horizontal_click_has_no_wall_so_the_face_picks_the_half() {
        assert!(!clicked(IVec3::Y, 0.9, Facing::North).top);
        assert!(clicked(IVec3::NEG_Y, 0.1, Facing::North).top);
    }

    #[test]
    fn toggling_swaps_the_collision_slab_between_flat_and_upright() {
        let (mut w, pos) = world_with_a_trapdoor(false);
        let flat = w.data.collision_boxes_at(pos.x, pos.y, pos.z)[0];
        assert!(flat.max[1] - flat.min[1] < 0.5, "closed: a flat panel");

        assert_eq!(w.toggle_trapdoor(pos), Some(true));
        let upright = w.data.collision_boxes_at(pos.x, pos.y, pos.z)[0];
        assert!(
            (upright.max[1] - upright.min[1] - 1.0).abs() < 1e-5,
            "open: standing full height"
        );
        assert!(upright.max[2] - upright.min[2] < 0.5, "open: thin on Z");

        assert_eq!(w.toggle_trapdoor(pos), Some(false));
        assert!(!w.trapdoor_state_at(pos.x, pos.y, pos.z).unwrap().open);
    }
}
