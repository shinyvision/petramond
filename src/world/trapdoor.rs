//! Trapdoors at the world level: the per-cell state lookup the position-aware
//! collision/selection reads, plus the open/close toggle and the per-frame
//! gather the dynamic renderer draws from.
//!
//! A trapdoor is ONE cell holding a [`TrapdoorState`]; placement and breaking
//! need nothing of their own (the generic single-cell paths already write and
//! clear the cell state). Like the door it is NOT chunk-meshed — it is drawn
//! dynamically and its collision is read live from the state, so a toggle needs
//! no remesh.

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::tile::Tile;
use petramond_world::trapdoor::TrapdoorState;

use super::store::World;

impl World {
    /// Gather the trapdoors to draw this frame: one entry per cell, as
    /// `(world pos, state, [top_art, bottom_art, edge], skylight, blocklight)`.
    /// Mirrors [`collect_doors`](Self::collect_doors); the swing angle is
    /// paired in later from the client's panel swings.
    pub fn collect_trapdoors(
        &self,
        out: &mut Vec<(
            IVec3,
            TrapdoorState,
            [Tile; 3],
            u8,
            petramond_world::light::BlockLight6,
        )>,
    ) {
        out.clear();
        for sp in &self.block_entity_sections {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            if section.cell_states().is_empty() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
            for &key in section.cell_states().keys() {
                let (lx, ly, lz) = petramond_world::chunk::section_local(key as usize);
                // The gated wrapper answers `None` for every non-trapdoor state
                // sharing the unified map (doors, stairs, torch mounts, fronts).
                let Some(state) = section.trapdoor_state(lx, ly, lz) else {
                    continue;
                };
                let tiles = Block::from_id(section.block_raw(lx, ly, lz)).tiles();
                let pos = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                let sky = self.skylight6_at_world(pos.x, pos.y, pos.z);
                let block = petramond_world::light::BlockLight6::from_x2(
                    self.blocklight_rgb_at_world(pos.x, pos.y, pos.z),
                );
                out.push((pos, state, tiles, sky, block));
            }
        }
    }

    /// The trapdoor state at world `pos`, or `None` when no trapdoor is
    /// recorded there or the cell is unloaded.
    #[inline]
    pub fn trapdoor_state_at(&self, wx: i32, wy: i32, wz: i32) -> Option<TrapdoorState> {
        let (c, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        c.trapdoor_state(lx, ly, lz)
    }

    /// Toggle a trapdoor open/closed. Collision follows the logical state (read
    /// live), so the player falls through the instant it opens; the visual
    /// swing is eased separately by the renderer. No remesh — a trapdoor is not
    /// chunk-meshed. Returns the NEW open state, or `None` if `pos` isn't one.
    pub fn toggle_trapdoor(&mut self, pos: IVec3) -> Option<bool> {
        let mut state = self.trapdoor_state_at(pos.x, pos.y, pos.z)?;
        state.open = !state.open;
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.set_trapdoor_state(lx, ly, lz, state);
        }
        // A toggle rewrites the cell state with NO block-id write, so it never
        // passes the announce choke point — log its delta explicitly (the new
        // open bit reaches replicas with it) and invalidate the nav caches the
        // same way: an opened hatch is a way down NOW.
        if self.replication.replication_capture {
            self.record_block_delta(pos.x, pos.y, pos.z);
        }
        self.push_nav_change(pos);
        Some(state.open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::facing::Facing;
    use petramond_world::block::{CellCodec, CellView, ShapeFamily};
    use petramond_world::chunk::{Chunk, ChunkPos};
    use petramond_world::world::placement::PlacementPlan;

    const TRAPDOOR: Block = Block::OakTrapdoor;

    fn world_with_a_trapdoor(top: bool) -> (World, IVec3) {
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let pos = IVec3::new(5, 64, 5);
        let state = TrapdoorState {
            facing: Facing::South,
            open: false,
            top,
        };
        // The GENERIC commit every family's placement lands through — not a
        // bespoke writer — so this covers the path a real place click takes.
        assert!(w.commit_placement(&PlacementPlan::single(pos, TRAPDOOR, state.to_cell()), true));
        (w, pos)
    }

    #[test]
    fn a_placed_trapdoor_reaches_the_render_gather() {
        // The gather walks the block-entity section index, so a placed panel
        // that never enters that index is invisible however correct its state
        // is — the failure mode a shape drawn outside the chunk mesh has.
        for top in [false, true] {
            let (w, pos) = world_with_a_trapdoor(top);
            assert_eq!(TRAPDOOR.shape_family(), ShapeFamily::Trapdoor);
            let mut rows = Vec::new();
            w.collect_trapdoors(&mut rows);
            assert_eq!(rows.len(), 1, "top={top}: one gathered panel");
            assert_eq!(rows[0].0, pos);
            assert_eq!(rows[0].1.top, top);
            assert!(!rows[0].1.open);
        }
    }

    /// The state a click on `hit`'s `normal` face, landing `spot_y` up that
    /// face, plans — through the shared placement ladder, so this is the rule
    /// the server write and the client ghost both run.
    fn clicked(normal: IVec3, spot_y: f32, player_facing: Facing) -> TrapdoorState {
        let mut w = World::new(1, 4);
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
        // Clicking a block's east face builds to its east, so the block is
        // WEST of the panel and the panel hinges west — never on wherever the
        // placer happens to be looking from, and never on a rotation key.
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
        // Landing on a block's top lays the panel on the floor above it;
        // landing under one hangs it from that cell's ceiling.
        assert!(!clicked(IVec3::Y, 0.9, Facing::North).top);
        assert!(clicked(IVec3::NEG_Y, 0.1, Facing::North).top);
    }

    #[test]
    fn toggling_swaps_the_collision_slab_between_flat_and_upright() {
        let (mut w, pos) = world_with_a_trapdoor(false);
        let flat = w.collision_boxes_at(pos.x, pos.y, pos.z)[0];
        assert!(flat.max[1] - flat.min[1] < 0.5, "closed: a flat panel");

        assert_eq!(w.toggle_trapdoor(pos), Some(true));
        let upright = w.collision_boxes_at(pos.x, pos.y, pos.z)[0];
        assert!(
            (upright.max[1] - upright.min[1] - 1.0).abs() < 1e-5,
            "open: standing full height"
        );
        assert!(upright.max[2] - upright.min[2] < 0.5, "open: thin on Z");

        assert_eq!(w.toggle_trapdoor(pos), Some(false));
        assert!(!w.trapdoor_state_at(pos.x, pos.y, pos.z).unwrap().open);
    }
}
