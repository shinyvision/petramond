pub mod stream {
    pub use crate::world::{MemoryCensus, ServerWorld};
    pub use petramond_math::facing::Facing;

    pub fn tick(world: &mut ServerWorld, n: u32) {
        let recipes = petramond_world::crafting::Recipes::default();
        for _ in 0..n {
            world.game_tick(&recipes);
        }
    }

    /// Places a block at `place_pos` like a player click would: footprint, orientation, per-cell
    /// state, skips body-occupancy check.
    /// Preview tools need this for MODEL blocks - `set_block_world` just writes one cell, no
    /// facing/offset, you get a fragment.
    /// False if placement refuses (unknown row, blocked footprint).
    pub fn place_block(
        world: &mut ServerWorld,
        name: &str,
        place_pos: [i32; 3],
        player_facing: petramond_math::facing::Facing,
    ) -> bool {
        let Some(id) = super::block::id_by_name(name) else {
            return false;
        };
        let block = petramond_world::block::Block::from_id(id);
        let p = petramond_math::math::IVec3::new(place_pos[0], place_pos[1], place_pos[2]);
        let inputs = crate::world::placement::PlaceInputs {
            hit: p - petramond_math::math::IVec3::Y,
            normal: petramond_math::math::IVec3::Y,
            spot: [0.5; 3],
            place_pos: p,
            replacing_in_place: false,
            player_facing,
            held_rotation: crate::server::player::HeldRotation {
                item: None,
                rotation: 0,
            },
            held: None,
        };
        let Some(plan) = world.placement_plan(block, &inputs, &mut |_, _| false) else {
            return false;
        };
        world.commit_placement(&plan, true)
    }

    pub fn set_model_parts(world: &mut ServerWorld, pos: [i32; 3], parts: u32) -> bool {
        world.set_model_parts(
            petramond_math::math::IVec3::new(pos[0], pos[1], pos[2]),
            parts,
            None,
        )
    }
}

pub mod mods {
    pub struct WorldgenMods {
        _host: crate::modding::ModHost,
    }

    pub fn load(seed: u32) -> WorldgenMods {
        let mut host = crate::modding::ModHost::load(seed, &Default::default());
        let mut world = crate::world::ServerWorld::with_pool(
            seed,
            4,
            std::sync::Arc::new(crate::worker::JobPool::inline()),
        );
        let mut bus = crate::events::EventBus::default();
        let mut systems = crate::events::TickSystems::default();
        let mut sound = 1u64;
        host.initialize(&mut world, &mut bus, &mut systems, &mut sound);
        WorldgenMods { _host: host }
    }
}

pub mod recipes_query {
    pub fn process(
        catalog: &petramond_world::crafting::Recipes,
        class: &str,
        item_key: &str,
    ) -> Option<String> {
        let item = petramond_world::item::ItemType::by_key(item_key)?;
        catalog
            .process(class, item)
            .map(|s| s.item.key().to_owned())
    }
}

pub mod draw {
    pub use mod_api::DrawPrim;
}

pub mod gui {
    pub fn loaded_documents() -> Vec<(&'static str, usize)> {
        crate::gui::documents::loaded_documents()
    }
}

pub mod recipes {
    pub use petramond_world::crafting::{CraftingCatalog, CraftingRecipe, Recipes, UnlockIndex};

    pub fn load() -> Result<Recipes, String> {
        petramond_world::crafting::load_recipes_for(&Default::default())
    }

    pub fn opened_by_items(index: &UnlockIndex, item_names: &[&str]) -> Vec<String> {
        let obtained: petramond_world::item::ItemSet = item_names
            .iter()
            .filter_map(|name| petramond_world::item::ItemType::by_name(name))
            .collect();
        index.opened_by_all(&obtained).map(str::to_owned).collect()
    }
}

pub mod biome {
    pub use petramond_world::biome::Biome;
}

pub mod block {
    pub use petramond_world::block::Block;

    pub fn id_by_name(name: &str) -> Option<u16> {
        petramond_world::registry::names().blocks.id(name)
    }

    pub fn name_of(id: u16) -> Option<&'static str> {
        petramond_world::registry::names().blocks.name(id)
    }
}

pub mod atlas {
    pub use petramond_world::tile::{Tile, TileTint};
}

pub mod chunk {
    pub use petramond_world::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
}

#[cfg(feature = "tools")]
pub mod worldgen {
    pub use petramond_worldgen::preview::*;
}
