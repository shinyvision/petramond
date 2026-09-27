use crate::world::{ServerWorld, World, WorldSide};
use petramond_math::facing::Facing;
use petramond_math::math::IVec3;
use petramond_world::block::{Block, CellView};
use petramond_world::door::DoorState;

use petramond_world::world::query::door_support;

use super::cell_change::{CellChange, ChangeKind};

const UP: IVec3 = IVec3::new(0, 1, 0);

pub struct Door;

impl crate::world::engine_behavior::EngineBlockBehavior for Door {
    fn neighbor_update(&self, world: &mut ServerWorld, pos: IVec3) {
        if world.door_state_at(pos.x, pos.y, pos.z).is_none() || world.door_supported(pos) {
            return;
        }
        world.break_door_naturally(pos);
    }
}

pub static DOOR: Door = Door;

impl<S: WorldSide> World<S> {
    #[inline]
    pub fn door_state_at(&self, wx: i32, wy: i32, wz: i32) -> Option<DoorState> {
        let (c, lx, ly, lz) = self.data.chunk_at_world(wx, wy, wz)?;
        c.door_state(lx, ly, lz)
    }

    #[inline]
    fn set_door_state_world(&mut self, pos: IVec3, state: DoorState) {
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.set_door_state(lx, ly, lz, state);
        }
    }

    fn door_supported(&self, pos: IVec3) -> bool {
        let Some((lower, _)) = self.door_cells(pos) else {
            return false;
        };
        let floor = self.data.physics_block(lower.x, lower.y - 1, lower.z);
        door_support(floor)
    }

    fn break_door_naturally(&mut self, pos: IVec3) {
        let Some((lower, _)) = self.door_cells(pos) else {
            return;
        };
        let block = Block::from_id(self.data.chunk_block(lower.x, lower.y, lower.z));
        self.note_block_destroyed(lower, block);
        self.remove_compound(pos);
    }

    pub fn place_door(&mut self, base: IVec3, block: Block, facing: Facing) -> bool {
        if !DoorState::owns(block) {
            return false;
        }
        let upper = base + UP;
        if !self.materialize_section_at(base) || !self.materialize_section_at(upper) {
            return false;
        }
        let mut changes = Vec::with_capacity(2);
        for (cell, top) in [(base, false), (upper, true)] {
            if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(cell.x, cell.y, cell.z) {
                changes.push(CellChange::new(
                    cell,
                    c.block(lx, ly, lz),
                    ChangeKind::Place,
                ));
                c.set_block(lx, ly, lz, block);
                c.set_door_state(
                    lx,
                    ly,
                    lz,
                    DoorState {
                        facing,
                        open: false,
                        top,
                    },
                );
                c.modified = true;
            }
        }
        self.apply_cell_changes(&changes);
        true
    }

    #[inline]
    pub fn door_lower_cell(&self, wx: i32, wy: i32, wz: i32) -> Option<IVec3> {
        self.door_cells(IVec3::new(wx, wy, wz))
            .map(|(lower, _)| lower)
    }

    pub fn door_cells(&self, pos: IVec3) -> Option<(IVec3, IVec3)> {
        let state = self.door_state_at(pos.x, pos.y, pos.z)?;
        Some(if state.top {
            (pos - UP, pos)
        } else {
            (pos, pos + UP)
        })
    }

    pub fn toggle_door(&mut self, pos: IVec3) -> Option<IVec3> {
        let (lower, upper) = self.door_cells(pos)?;
        for c in [lower, upper] {
            if let Some(mut state) = self.door_state_at(c.x, c.y, c.z) {
                state.open = !state.open;
                self.set_door_state_world(c, state);
            }
            self.record_block_delta(c.x, c.y, c.z);
            self.push_nav_change(c);
        }
        Some(lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::{Chunk, ChunkPos};
    use petramond_world::crafting::Recipes;

    const DOOR: Block = Block::OakDoor;

    fn world_with_floor() -> (ServerWorld, IVec3) {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let base = IVec3::new(5, 64, 5);
        w.set_block_world(base.x, base.y - 1, base.z, Block::Stone);
        (w, base)
    }

    fn run_ticks(w: &mut ServerWorld, n: u32) {
        let r = Recipes::default();
        for _ in 0..n {
            w.game_tick(&r);
        }
    }

    #[test]
    fn placing_fills_both_cells_with_paired_state() {
        let (mut w, base) = world_with_floor();
        assert!(w.data.door_footprint_clear(base));
        assert!(w.place_door(base, DOOR, Facing::South));
        let upper = base + UP;
        assert_eq!(
            Block::from_id(w.data.chunk_block(base.x, base.y, base.z)),
            DOOR
        );
        assert_eq!(
            Block::from_id(w.data.chunk_block(upper.x, upper.y, upper.z)),
            DOOR
        );
        let lo = w.door_state_at(base.x, base.y, base.z).unwrap();
        let hi = w.door_state_at(upper.x, upper.y, upper.z).unwrap();
        assert_eq!(lo.facing, Facing::South);
        assert!(!lo.top && hi.top, "lower is bottom half, upper is top half");
        assert!(!lo.open && !hi.open, "a placed door starts closed");
    }

    #[test]
    fn placement_needs_a_floor_and_two_clear_cells() {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let base = IVec3::new(5, 64, 5);
        assert!(!w.data.door_footprint_clear(base));
        w.set_block_world(base.x, base.y - 1, base.z, Block::Stone);
        assert!(w.data.door_footprint_clear(base));
        w.set_block_world(base.x, base.y + 1, base.z, Block::Stone);
        assert!(!w.data.door_footprint_clear(base));
    }

    #[test]
    fn a_door_needs_an_opaque_floor_not_a_chest_workbench_or_cactus() {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let base = IVec3::new(5, 64, 5);
        w.set_block_world(base.x, base.y - 1, base.z, Block::Stone);
        assert!(w.data.door_footprint_clear(base));
        for floor in [Block::Chest, Block::FurnitureWorkbench, Block::Cactus] {
            w.set_block_world(base.x, base.y - 1, base.z, floor);
            assert!(
                !w.data.door_footprint_clear(base),
                "{floor:?} is not a valid door support",
            );
        }
    }

    #[test]
    fn a_door_breaks_with_the_update_that_undermines_its_floor() {
        let (mut w, base) = world_with_floor();
        let upper = base + UP;
        w.place_door(base, DOOR, Facing::South);
        run_ticks(&mut w, 2);
        assert_eq!(
            Block::from_id(w.data.chunk_block(base.x, base.y, base.z)),
            DOOR
        );

        w.set_block_world(base.x, base.y - 1, base.z, Block::Air);
        run_ticks(&mut w, 1);
        assert_eq!(
            Block::from_id(w.data.chunk_block(base.x, base.y, base.z)),
            Block::Air,
            "the undermined door's lower half breaks",
        );
        assert_eq!(
            Block::from_id(w.data.chunk_block(upper.x, upper.y, upper.z)),
            Block::Air,
            "and its upper half goes with it (the pair breaks as one)",
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks.iter().any(|&(p, b)| p == base && b == DOOR),
            "exactly the lower cell drops one door item",
        );
        assert_eq!(breaks.len(), 1, "a door drops once, not once per cell");
    }

    #[test]
    fn a_door_survives_a_change_that_leaves_its_floor_intact() {
        let (mut w, base) = world_with_floor();
        w.place_door(base, DOOR, Facing::South);
        run_ticks(&mut w, 2);
        w.set_block_world(base.x + 1, base.y, base.z, Block::Stone);
        w.set_block_world(base.x + 1, base.y, base.z, Block::Air);
        run_ticks(&mut w, 3);
        assert_eq!(
            Block::from_id(w.data.chunk_block(base.x, base.y, base.z)),
            DOOR
        );
        assert!(w.take_natural_breaks().is_empty());
    }

    #[test]
    fn toggling_swaps_collision_and_breaking_removes_the_pair() {
        let (mut w, base) = world_with_floor();
        w.place_door(base, DOOR, Facing::South);
        let upper = base + UP;

        let closed = w.data.collision_boxes_at(base.x, base.y, base.z)[0];
        assert!(closed.max[2] - closed.min[2] < 0.5);

        assert_eq!(w.toggle_door(upper), Some(base));
        for cell in [base, upper] {
            let open = w.data.collision_boxes_at(cell.x, cell.y, cell.z)[0];
            assert!(
                open.max[0] - open.min[0] < 0.5,
                "open slab should be thin on X"
            );
            assert!(w.door_state_at(cell.x, cell.y, cell.z).unwrap().open);
        }

        let removed = w.remove_compound(base).unwrap();
        assert_eq!(removed.len(), 2);
        for c in removed {
            assert_eq!(
                Block::from_id(w.data.chunk_block(c.x, c.y, c.z)),
                Block::Air
            );
            assert!(w.door_state_at(c.x, c.y, c.z).is_none());
        }
    }
}
