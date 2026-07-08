//! Wooden doors at the world level: the per-cell state lookup the position-aware
//! collision/selection in [`model`](super::model) reads, plus 2-cell placement, the
//! whole-door break, and the open/close toggle.
//!
//! A door spans two stacked cells, each holding its own [`DoorState`] in the chunk
//! door map (the upper carries `top = true`). Placement/break/toggle all operate over
//! the pair so the door behaves as one object — mirroring how `model` treats a
//! bbmodel block's footprint. The door is NOT chunk-meshed (it is drawn dynamically,
//! see `render::door_model`), and its collision is read live from the door state, so a
//! toggle needs no remesh — only the placement/break edits relight + remesh neighbours.

use crate::atlas::Tile;
use crate::block::{Block, BlockBehavior, RenderShape};
use crate::door::DoorState;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::store::World;

/// Cell offset from a door's lower cell to its upper cell.
const UP: IVec3 = IVec3::new(0, 1, 0);

/// Ticks a now-unsupported door waits before it breaks — the next tick, the same
/// scheduled-break model fragile blocks and water use (see [`Door`]).
const DOOR_BREAK_DELAY: u64 = 1;

/// Whether `floor` can hold up a door: a FULL OPAQUE block. This is the one rule both
/// placement ([`World::door_footprint_clear`]) and the break behaviour ([`Door`]) read,
/// so the two agree — and it matches the torch/fragile support test. Chests, the
/// furniture workbench and cactuses are SOLID but NOT opaque (non-full-cube models), so
/// a door refuses to stand on them and falls if its opaque floor is dug out.
fn door_support(floor: Block) -> bool {
    floor.is_opaque()
}

/// Break behaviour for doors: a neighbour change that takes away the floor under the
/// door's LOWER cell schedules the whole door to break next tick; the scheduled tick
/// re-checks (the floor may have returned, or the cell may now hold something else) and,
/// only if it is still an unsupported door, shatters the pair — dropping ONE door item
/// and bursting, exactly as a hand-break would. Mirrors [`Fragile`](super::fragile), but
/// resolves over the 2-cell door so the upper half (which rests on the lower, not on an
/// opaque block) is never mistaken for unsupported.
pub struct Door;

impl BlockBehavior for Door {
    fn key(&self) -> &'static str {
        "door"
    }

    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        if !world.door_supported(pos) {
            world.schedule_block_tick(pos, DOOR_BREAK_DELAY);
        }
    }

    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        // The cell may have changed since the break was scheduled (mined, re-supported,
        // or replaced); only break a door that is still there and still unsupported.
        if world.door_state_at(pos.x, pos.y, pos.z).is_none() || world.door_supported(pos) {
            return;
        }
        world.break_door_naturally(pos);
    }
}

/// The door singleton a row points at (`behavior: &behavior::DOOR`).
pub static DOOR: Door = Door;

impl World {
    /// Gather the doors to draw this frame: one entry per door (its LOWER cell), as
    /// `(lower world pos, state, [bottom_art, top_art, side], skylight)`. The upper cell
    /// is skipped (the renderer builds both halves from the lower entry). Mirrors
    /// [`collect_chests`](Self::collect_chests); the swing angle is paired in later from
    /// `Game::door_swing_angle`.
    pub fn collect_doors(&self, out: &mut Vec<(IVec3, DoorState, [Tile; 3], u8, u8)>) {
        out.clear();
        for sp in &self.block_entity_sections {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            let doors = section.doors();
            if doors.is_empty() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
            for (&key, &state) in doors {
                if state.top {
                    continue; // emit once per door, from its lower cell
                }
                // Invert the section-local block index (idx = y*256 + z*16 + x).
                let lx = (key & 0x0F) as usize;
                let lz = ((key >> 4) & 0x0F) as usize;
                let ly = (key >> 8) as usize;
                // The door BlockDef row's [top, bottom, side] tiles: front-face art for
                // each half, plus the distinct edge tile.
                let [top, bottom, side] = Block::from_id(section.block_raw(lx, ly, lz)).tiles();
                let pos = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                let sky = self.skylight6_at_world(pos.x, pos.y, pos.z);
                let block = self.blocklight6_at_world(pos.x, pos.y, pos.z);
                out.push((pos, state, [bottom, top, side], sky, block));
            }
        }
    }

    /// The door state (facing + open + which-half) at world `pos`, or `None` when no
    /// door is recorded there or the cell is unloaded. Read by the position-aware
    /// collision/selection (see [`collision_boxes_at`](Self::collision_boxes_at)) and
    /// the dynamic door renderer.
    #[inline]
    pub fn door_state_at(&self, wx: i32, wy: i32, wz: i32) -> Option<DoorState> {
        let (c, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        c.door_state(lx, ly, lz)
    }

    #[inline]
    fn set_door_state_world(&mut self, pos: IVec3, state: DoorState) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.set_door_state(lx, ly, lz, state);
        }
    }

    /// Whether a 2-tall door with its lower cell at `base` can be placed: both cells
    /// are loaded + replaceable, and the cell directly below has something to stand on
    /// (so a door never floats). The caller adds the entity-overlap gate.
    pub fn door_footprint_clear(&self, base: IVec3) -> bool {
        let upper = base + UP;
        let floor = self.physics_block(base.x, base.y - 1, base.z);
        self.placement_cell_open(base) && self.placement_cell_open(upper) && door_support(floor)
    }

    /// Whether the door `pos` belongs to still has a valid floor under its LOWER cell
    /// (a full opaque block, per [`door_support`]). The upper half's "floor" is the
    /// lower door cell, which is never opaque, so we always resolve to the lower cell
    /// first — both halves share the one support test. `false` when `pos` isn't a door
    /// (the scheduled break guards that separately).
    fn door_supported(&self, pos: IVec3) -> bool {
        let Some((lower, _)) = self.door_cells(pos) else {
            return false;
        };
        let floor = self.physics_block(lower.x, lower.y - 1, lower.z);
        door_support(floor)
    }

    /// Break the door `pos` belongs to the way the sim breaks an undermined fragile
    /// block: record ONE natural break (burst + a single door drop) at its lower cell,
    /// then remove both halves. `Game` drains the break to play the burst and roll the
    /// drop (see [`World::take_natural_breaks`]).
    fn break_door_naturally(&mut self, pos: IVec3) {
        let Some((lower, _)) = self.door_cells(pos) else {
            return;
        };
        let block = Block::from_id(self.chunk_block(lower.x, lower.y, lower.z));
        self.note_block_destroyed(lower, block);
        self.remove_door(pos);
    }

    /// Place a 2-tall `block` door with its lower cell at `base`, on `facing`'s edge.
    /// Writes the door id + per-cell [`DoorState`] (lower `top = false`, upper `top =
    /// true`) to both cells, then relights + remeshes the region (the door isn't
    /// chunk-meshed, but its neighbours are). Assumes the footprint was gated clear.
    /// Returns false if `block` isn't a door or a cell is unloaded.
    pub fn place_door(&mut self, base: IVec3, block: Block, facing: Facing) -> bool {
        if block.render_shape() != RenderShape::Door {
            return false;
        }
        let upper = base + UP;
        // Materialize the (possibly all-air, hence absent) sections the door occupies so
        // the writes land; bail only if a cell is outside the world's vertical range.
        if !self.materialize_section_at(base) || !self.materialize_section_at(upper) {
            return false;
        }
        for (cell, top) in [(base, false), (upper, true)] {
            if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(cell.x, cell.y, cell.z) {
                // `set_block` clears any stale door entry; then record this cell's state.
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
            self.note_block_entity_change(cell);
        }
        self.refresh_region(&[base, upper]);
        true
    }

    /// The LOWER cell of the door at world `pos` (the cell itself if it is the bottom
    /// half, else the cell below). `None` if `pos` isn't a door. The animation keys on
    /// the lower cell, so the toggle path resolves it before flipping the state.
    #[inline]
    pub fn door_lower_cell(&self, wx: i32, wy: i32, wz: i32) -> Option<IVec3> {
        self.door_cells(IVec3::new(wx, wy, wz))
            .map(|(lower, _)| lower)
    }

    /// The (lower, upper) cells of the door `pos` belongs to, found via the recorded
    /// `top` bit. `None` if `pos` isn't a door cell.
    fn door_cells(&self, pos: IVec3) -> Option<(IVec3, IVec3)> {
        let state = self.door_state_at(pos.x, pos.y, pos.z)?;
        Some(if state.top {
            (pos - UP, pos)
        } else {
            (pos, pos + UP)
        })
    }

    /// Break the whole door `pos` belongs to: set both cells to air (clearing their
    /// door state) and relight + remesh. Returns the removed cells (so the caller can
    /// spawn ONE drop + a burst), or `None` if `pos` isn't a door cell.
    pub fn remove_door(&mut self, pos: IVec3) -> Option<Vec<IVec3>> {
        let (lower, upper) = self.door_cells(pos)?;
        for c in [lower, upper] {
            if let Some((chunk, lx, ly, lz)) = self.chunk_at_world_mut(c.x, c.y, c.z) {
                chunk.set_block(lx, ly, lz, Block::Air); // also clears the door state
                chunk.modified = true;
            }
            self.note_block_entity_change(c);
        }
        self.refresh_region(&[lower, upper]);
        Some(vec![lower, upper])
    }

    /// Toggle a door open/closed: flip `open` on BOTH cells. Collision follows the
    /// logical state (read live from [`door_state_at`](Self::door_state_at)), so the
    /// player can walk through the instant it opens; the visual swing is eased
    /// separately by the renderer. No remesh — the door isn't chunk-meshed. Returns the
    /// lower cell (to key the animation), or `None` if `pos` isn't a door.
    pub fn toggle_door(&mut self, pos: IVec3) -> Option<IVec3> {
        let (lower, upper) = self.door_cells(pos)?;
        for c in [lower, upper] {
            if let Some(mut state) = self.door_state_at(c.x, c.y, c.z) {
                state.open = !state.open;
                self.set_door_state_world(c, state);
            }
            // A toggle flips the door map with NO block-id write, so it never
            // passes the announce choke point — log its delta explicitly
            // (`state: Some(Door(..))` carries the new open bit to replicas).
            if self.replication_capture {
                self.record_block_delta(c.x, c.y, c.z);
            }
        }
        Some(lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::crafting::Recipes;

    const DOOR: Block = Block::OakDoor;

    fn world_with_floor() -> (World, IVec3) {
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let base = IVec3::new(5, 64, 5);
        // A solid floor so the door has something to stand on.
        w.set_block_world(base.x, base.y - 1, base.z, Block::Stone);
        (w, base)
    }

    fn run_ticks(w: &mut World, n: u32) {
        let r = Recipes::default();
        for _ in 0..n {
            w.game_tick(&r);
        }
    }

    #[test]
    fn placing_fills_both_cells_with_paired_state() {
        let (mut w, base) = world_with_floor();
        assert!(w.door_footprint_clear(base));
        assert!(w.place_door(base, DOOR, Facing::South));
        let upper = base + UP;
        assert_eq!(Block::from_id(w.chunk_block(base.x, base.y, base.z)), DOOR);
        assert_eq!(
            Block::from_id(w.chunk_block(upper.x, upper.y, upper.z)),
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
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let base = IVec3::new(5, 64, 5);
        // No floor below: a door can't float.
        assert!(!w.door_footprint_clear(base));
        // Add a floor — now clear.
        w.set_block_world(base.x, base.y - 1, base.z, Block::Stone);
        assert!(w.door_footprint_clear(base));
        // Block the upper cell — the 2-tall footprint is no longer clear.
        w.set_block_world(base.x, base.y + 1, base.z, Block::Stone);
        assert!(!w.door_footprint_clear(base));
    }

    #[test]
    fn a_door_needs_an_opaque_floor_not_a_chest_workbench_or_cactus() {
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let base = IVec3::new(5, 64, 5);
        // A full opaque block holds a door up.
        w.set_block_world(base.x, base.y - 1, base.z, Block::Stone);
        assert!(w.door_footprint_clear(base));
        // Chests, the furniture workbench and cactuses are SOLID but NOT opaque (partial
        // models), so a door refuses to stand on them.
        for floor in [Block::Chest, Block::FurnitureWorkbench, Block::Cactus] {
            w.set_block_world(base.x, base.y - 1, base.z, floor);
            assert!(
                !w.door_footprint_clear(base),
                "{floor:?} is not a valid door support",
            );
        }
    }

    #[test]
    fn a_door_breaks_the_tick_after_its_floor_is_dug_away() {
        let (mut w, base) = world_with_floor();
        let upper = base + UP;
        w.place_door(base, DOOR, Facing::South);
        run_ticks(&mut w, 2); // settle: supported, nothing happens
        assert_eq!(Block::from_id(w.chunk_block(base.x, base.y, base.z)), DOOR);

        // Dig the floor out from under it: the door is scheduled, then breaks next tick.
        w.set_block_world(base.x, base.y - 1, base.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            Block::from_id(w.chunk_block(base.x, base.y, base.z)),
            Block::Air,
            "the undermined door's lower half breaks",
        );
        assert_eq!(
            Block::from_id(w.chunk_block(upper.x, upper.y, upper.z)),
            Block::Air,
            "and its upper half goes with it (the pair breaks as one)",
        );
        // It was handed to the presentation layer as ONE natural break at the lower cell.
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
        // A block placed/removed beside the door (its floor untouched) must not break it.
        w.set_block_world(base.x + 1, base.y, base.z, Block::Stone);
        w.set_block_world(base.x + 1, base.y, base.z, Block::Air);
        run_ticks(&mut w, 3);
        assert_eq!(Block::from_id(w.chunk_block(base.x, base.y, base.z)), DOOR);
        assert!(w.take_natural_breaks().is_empty());
    }

    #[test]
    fn toggling_swaps_collision_and_breaking_removes_the_pair() {
        let (mut w, base) = world_with_floor();
        w.place_door(base, DOOR, Facing::South);
        let upper = base + UP;

        // Closed: the slab is thin on Z (sits on the south edge).
        let closed = w.collision_boxes_at(base.x, base.y, base.z)[0];
        assert!(closed.max[2] - closed.min[2] < 0.5);

        // Toggle from the UPPER cell flips BOTH halves to open (thin on X now).
        assert_eq!(w.toggle_door(upper), Some(base));
        for cell in [base, upper] {
            let open = w.collision_boxes_at(cell.x, cell.y, cell.z)[0];
            assert!(
                open.max[0] - open.min[0] < 0.5,
                "open slab should be thin on X"
            );
            assert!(w.door_state_at(cell.x, cell.y, cell.z).unwrap().open);
        }

        // Breaking either cell clears the whole door.
        let removed = w.remove_door(base).unwrap();
        assert_eq!(removed.len(), 2);
        for c in removed {
            assert_eq!(Block::from_id(w.chunk_block(c.x, c.y, c.z)), Block::Air);
            assert!(w.door_state_at(c.x, c.y, c.z).is_none());
        }
    }
}
