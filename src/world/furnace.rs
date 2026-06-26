//! Furnace block-entities at the world level: the per-tick smelting fan-out and
//! world-coordinate access to the chunk-owned furnace maps.
//!
//! Furnaces live on their chunk (see [`crate::chunk::Chunk`]), so these are thin
//! world↔chunk coordinate wrappers plus the tick driver that supplies the recipe
//! set the storage layer is kept ignorant of.

use crate::chunk::{ChunkPos, CHUNK_SX, CHUNK_SZ};
use crate::crafting::Recipes;
use crate::furnace::{Facing, Furnace};
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// Advance every loaded furnace by one game tick, smelting per `recipes`.
    /// Furnaces are chunk-owned, so this fans out to each chunk, then promotes
    /// lit-state flips to world-coordinate mesh invalidation and block updates.
    /// Cheap for the common furnace-free chunk (an empty-map early-out).
    ///
    /// One step of the per-tick sequence owned by [`World::game_tick`]; not a
    /// public entry point.
    pub(super) fn tick_furnaces(&mut self, recipes: &Recipes) {
        let mut relit = Vec::new();
        for (&cpos, chunk) in self.chunks.iter_mut() {
            for (lx, ly, lz) in chunk.tick_furnaces(|it| recipes.smelt(it)) {
                relit.push((cpos, local_to_world(cpos, lx, ly, lz)));
            }
        }

        for (cpos, pos) in relit {
            self.mark_dirty_pos(cpos);
            // A furnace's lit-state flip changes its block-light emission. The
            // announce re-floods the 3x3 light neighbourhood, and that relight then
            // re-meshes every chunk the glow reaches (same path a torch placement
            // takes).
            self.notify_block_and_neighbors(pos.x, pos.y, pos.z);
        }
    }

    /// The furnace at a world block position, if one is stored there.
    pub fn furnace_at(&self, pos: IVec3) -> Option<&Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.furnace_at(lx, ly, lz)
    }

    /// Mutable handle to the furnace at a world block position (GUI edits).
    pub fn furnace_at_mut(&mut self, pos: IVec3) -> Option<&mut Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.furnace_at_mut(lx, ly, lz)
    }

    /// Install an empty furnace facing `facing` at a freshly placed furnace block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_furnace(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_furnace(
                lx,
                ly,
                lz,
                Furnace {
                    facing,
                    ..Furnace::default()
                },
            );
        }
    }

    /// Remove and return the furnace at a world position (block break), if any.
    pub fn take_furnace(&mut self, pos: IVec3) -> Option<Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.take_furnace(lx, ly, lz)
    }
}

#[inline]
fn local_to_world(cpos: ChunkPos, lx: usize, ly: usize, lz: usize) -> IVec3 {
    IVec3::new(
        cpos.cx * CHUNK_SX as i32 + lx as i32,
        ly as i32,
        cpos.cz * CHUNK_SZ as i32 + lz as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::atlas::Tile;
    use crate::block::Block;
    use crate::chunk::Chunk;
    use crate::crafting::SmeltingRecipe;
    use crate::item::{ItemStack, ItemType};
    use crate::mesh::{compute_chunk_skylight, ChunkMesh};

    fn furnace_recipes() -> Recipes {
        Recipes::new(
            Vec::new(),
            vec![SmeltingRecipe {
                input: ItemType::RawIron,
                result: ItemStack::new(ItemType::IronIngot, 1),
            }],
        )
    }

    fn fueled_furnace() -> Furnace {
        Furnace {
            input: Some(ItemStack::new(ItemType::RawIron, 1)),
            fuel: Some(ItemStack::new(ItemType::Coal, 1)),
            facing: Facing::East,
            ..Default::default()
        }
    }

    fn block(world: &World, x: i32, y: i32, z: i32) -> Block {
        Block::from_id(world.chunk_block(x, y, z))
    }

    fn count_tile(mesh: &ChunkMesh, tile: Tile) -> usize {
        mesh.opaque
            .iter()
            .filter(|v| v.packed & 0xFF == tile as u32)
            .count()
    }

    #[test]
    fn furnace_lit_flip_queues_remesh_for_texture_swap() {
        let pos = ChunkPos::new(0, 0);
        let mut chunk = Chunk::new(pos.cx, pos.cz);
        chunk.set_block(8, 64, 8, Block::Furnace);
        chunk.insert_furnace(8, 64, 8, fueled_furnace());
        let (band, ylo, yhi) = compute_chunk_skylight(&chunk);
        chunk.set_skylight(band, ylo, yhi);
        chunk.dirty = false;

        let mut world = World::new(0, 0);
        world.insert_chunk_for_test(pos, chunk);
        world.tick_mesh_budget(1);
        let mesh = world.meshes.get(&pos).expect("initial mesh built");
        assert_eq!(count_tile(mesh, Tile::FurnaceFront), 4);
        assert_eq!(count_tile(mesh, Tile::FurnaceFrontOn), 0);

        world.game_tick(&furnace_recipes());
        // A lit furnace now emits block light, so the lit-flip re-dirties this chunk's
        // light (this dirtying IS the new behavior). That would otherwise defer the
        // texture-swap remesh behind the async block-light bake, so re-settle the
        // light synchronously here — exactly as the test does before the initial mesh.
        let (band, ylo, yhi) = compute_chunk_skylight(world.chunks.get(&pos).unwrap());
        world
            .chunks
            .get_mut(&pos)
            .unwrap()
            .set_skylight(band, ylo, yhi);
        world.tick_mesh_budget(1);

        let mesh = world.meshes.get(&pos).expect("relit mesh rebuilt");
        assert_eq!(count_tile(mesh, Tile::FurnaceFront), 0);
        assert_eq!(count_tile(mesh, Tile::FurnaceFrontOn), 4);
    }

    #[test]
    fn furnace_lit_flip_emits_neighbor_block_update() {
        let pos = ChunkPos::new(0, 0);
        let mut chunk = Chunk::new(pos.cx, pos.cz);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Stone);
            }
        }
        chunk.set_block(8, 65, 8, Block::Water);
        chunk.set_block(9, 65, 8, Block::Furnace);
        chunk.insert_furnace(9, 65, 8, fueled_furnace());

        let mut world = World::new(0, 0);
        world.insert_chunk_for_test(pos, chunk);
        let recipes = furnace_recipes();

        world.game_tick(&recipes); // the furnace lights and queues block updates
        world.game_tick(&recipes); // the water receives the update and schedules flow
        assert_eq!(block(&world, 7, 65, 8), Block::Air);

        for _ in 0..10 {
            world.game_tick(&recipes);
        }

        assert_eq!(
            block(&world, 7, 65, 8),
            Block::Water,
            "adjacent water should flow after the furnace's block update"
        );
    }
}
