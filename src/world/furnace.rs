//! Furnace block-entities at the world level: the per-tick smelting fan-out and
//! world-coordinate access to the section-owned furnace state.
//!
//! A furnace is machine state ([`Furnace`]) plus slots in the block's generic
//! [`Container`](crate::container::Container) plus an entity facing — three
//! sibling section maps under one key. These are thin world↔section coordinate
//! wrappers plus the tick driver that supplies the recipe set the storage
//! layer is kept ignorant of.

use crate::chunk::{SectionPos, SECTION_SIZE};
use crate::container::Container;
use crate::crafting::Recipes;
use crate::facing::Facing;
use crate::furnace::{Furnace, FURNACE_SLOTS};
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// Advance every loaded furnace by one game tick, smelting per `recipes`.
    /// Furnaces are section-owned, so this fans out to each section, then
    /// promotes lit-state flips to world-coordinate mesh invalidation and block
    /// updates. Cheap for the common furnace-free section (an empty-map
    /// early-out).
    ///
    /// One step of the per-tick sequence owned by [`World::game_tick`]; not a
    /// public entry point.
    pub(super) fn tick_furnaces(&mut self, recipes: &Recipes) {
        let mut relit = Vec::new();
        // Only indexed sections can hold a furnace; skip the Arc::make_mut
        // (a potential copy-on-write clone) for chest/door-only ones. Sorted:
        // set order reflects streaming history, and the relit block updates
        // must fire in a deterministic order (the multiplayer tick contract).
        let mut candidates: Vec<_> = self.block_entity_sections.iter().copied().collect();
        candidates.sort_unstable_by_key(|p| (p.cx, p.cy, p.cz));
        for cpos in candidates {
            let Some(section) = self.sections.get_mut(&cpos) else {
                continue;
            };
            if section.furnaces().is_empty() {
                continue;
            }
            let section = std::sync::Arc::make_mut(section);
            for (lx, ly, lz) in section.tick_furnaces(|it| recipes.smelt(it)) {
                relit.push((cpos, local_to_world(cpos, lx, ly, lz)));
            }
        }

        for (cpos, pos) in relit {
            self.queue_dirty_mesh(cpos);
            // A furnace's lit-state flip changes its block-light emission. The
            // announce re-floods the 3x3 light neighbourhood, and that relight then
            // re-meshes every chunk the glow reaches (same path a torch placement
            // takes).
            self.notify_block_and_neighbors(pos.x, pos.y, pos.z);
        }
    }

    /// The furnace state at a world block position, if one is stored there.
    pub fn furnace_at(&self, pos: IVec3) -> Option<&Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.furnace_at(lx, ly, lz)
    }

    /// The furnace state and its container slots at a world position,
    /// split-borrowed for GUI edits and the furnace view.
    pub fn furnace_parts_mut(&mut self, pos: IVec3) -> Option<(&mut Furnace, &mut Container)> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.furnace_parts_mut(lx, ly, lz)
    }

    /// Install an empty furnace facing `facing` at a freshly placed furnace
    /// block: default machine state, an empty 3-slot container, and the
    /// facing. No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_furnace(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_furnace(lx, ly, lz, Furnace::default());
            c.insert_container(lx, ly, lz, Container::with_len(FURNACE_SLOTS));
            c.insert_entity_facing(lx, ly, lz, facing);
            self.note_block_entity_change(pos);
        }
    }

}

#[inline]
fn local_to_world(cpos: SectionPos, lx: usize, ly: usize, lz: usize) -> IVec3 {
    IVec3::new(
        cpos.cx * SECTION_SIZE as i32 + lx as i32,
        cpos.cy * SECTION_SIZE as i32 + ly as i32,
        cpos.cz * SECTION_SIZE as i32 + lz as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::atlas::Tile;
    use crate::block::Block;
    use crate::chunk::{SectionPos, SECTION_VOLUME};
    use crate::crafting::{ProcessingRecipe, SMELTING_CLASS};
    use crate::furnace::{SLOT_FUEL, SLOT_INPUT};
    use crate::item::{ItemStack, ItemType};
    use crate::mesh::ChunkMesh;
    use crate::section::Section;

    fn furnace_recipes() -> Recipes {
        Recipes::new(
            Vec::new(),
            vec![ProcessingRecipe {
                class: SMELTING_CLASS.to_owned(),
                input: ItemType::RawIron,
                result: ItemStack::new(ItemType::IronIngot, 1),
            }],
            Vec::new(),
        )
    }

    /// Install a fueled furnace (state + slots + facing) at a section-local cell.
    fn insert_fueled_furnace(section: &mut Section, x: usize, y: usize, z: usize) {
        section.insert_furnace(x, y, z, Furnace::default());
        let mut container = Container::with_len(FURNACE_SLOTS);
        container.slots[SLOT_INPUT] = Some(ItemStack::new(ItemType::RawIron, 1));
        container.slots[SLOT_FUEL] = Some(ItemStack::new(ItemType::Coal, 1));
        section.insert_container(x, y, z, container);
        section.insert_entity_facing(x, y, z, Facing::East);
    }

    fn block(world: &World, x: i32, y: i32, z: i32) -> Block {
        Block::from_id(world.chunk_block(x, y, z))
    }

    fn count_tile(mesh: &ChunkMesh, tile: Tile) -> usize {
        mesh.opaque
            .iter()
            .filter(|v| v.packed & 0xFF == tile.index() as u32)
            .count()
    }

    /// Clear a section's `light_dirty` flag by installing a settled (all-zero) skylight
    /// cube, so `tick_mesh_budget` builds its mesh now instead of deferring behind the
    /// async light bake. The furnace tile count is what's under test, not the light
    /// value, so a zero cube is fine.
    fn settle_section_light(world: &mut World, wx: i32, wy: i32, wz: i32) {
        world
            .section_at_world_mut_for_test(wx, wy, wz)
            .expect("section loaded")
            .set_skylight(vec![0u8; SECTION_VOLUME].into());
    }

    #[test]
    fn furnace_lit_flip_queues_remesh_for_texture_swap() {
        // Build just the furnace's section (0,4,0) — world (8,64,8) → section-local
        // (8,0,8) — so the mesh budget isn't spent on a column's other sections.
        let spos = SectionPos::new(0, 4, 0);
        let mut section = Section::new(spos.cx, spos.cy, spos.cz);
        section.set_block(8, 0, 8, Block::Furnace);
        insert_fueled_furnace(&mut section, 8, 0, 8);
        section.set_skylight(vec![0u8; SECTION_VOLUME].into()); // settle light

        let mut world = World::new(0, 0);
        world.insert_section_for_test(spos, section);
        world.mesh_section_blocking_for_test(spos);
        let mesh = world.meshes.get(&spos).expect("initial mesh built");
        assert_eq!(count_tile(mesh, Tile::named("furnace_front")), 4);
        assert_eq!(count_tile(mesh, Tile::named("furnace_front_on")), 0);

        world.game_tick(&furnace_recipes());
        // A lit furnace now emits block light, so the lit-flip re-dirties this section's
        // light (this dirtying IS the new behavior). That would otherwise defer the
        // texture-swap remesh behind the async light bake, so re-settle the light
        // synchronously here — exactly as the test does before the initial mesh.
        settle_section_light(&mut world, 8, 64, 8);
        world.mesh_section_blocking_for_test(spos);

        let mesh = world.meshes.get(&spos).expect("relit mesh rebuilt");
        assert_eq!(count_tile(mesh, Tile::named("furnace_front")), 0);
        assert_eq!(count_tile(mesh, Tile::named("furnace_front_on")), 4);
    }

    #[test]
    fn furnace_lit_flip_emits_neighbor_block_update() {
        // Build the section directly so the furnace BLOCK-ENTITY is present: a column
        // `Chunk` fixture carries blocks + water through the split, but not block-entities
        // (real worldgen produces none), so a pre-placed furnace's fuel would be lost.
        // Section (0,4,0) → world y 64..79; floor at local y 0 (world 64).
        let spos = SectionPos::new(0, 4, 0);
        let mut section = Section::new(spos.cx, spos.cy, spos.cz);
        for z in 0..16 {
            for x in 0..16 {
                section.set_block(x, 0, z, Block::Stone); // floor at world y 64
            }
        }
        section.set_block(8, 1, 8, Block::Water); // source water at world (8,65,8)
        section.set_block(9, 1, 8, Block::Furnace);
        insert_fueled_furnace(&mut section, 9, 1, 8); // world (9,65,8)

        let mut world = World::new(0, 0);
        world.insert_section_for_test(spos, section);
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
