//! Explicit facade for package preview tools.
//!
//! The game/runtime modules stay crate-internal; binaries under `src/bin` are
//! separate crates, so they use this narrow surface.

/// The live `World`, for dev tools that must observe or drive real streaming
/// rather than a re-derivation of it — generation/light/mesh pumping, the
/// resident-memory census, and deterministic ticking.
pub mod stream {
    pub use crate::facing::Facing;
    pub use crate::world::{MemoryCensus, World};

    /// Run `n` deterministic game ticks over a streamed world.
    ///
    /// A containment audit has no other honest way to ask "does this water
    /// move": the fluid sim only ever acts on the tick, and re-deriving its
    /// spread rules in a tool would just be a mirror that can go stale.
    pub fn tick(world: &mut World, n: u32) {
        let recipes = crate::crafting::Recipes::default();
        for _ in 0..n {
            world.game_tick(&recipes);
        }
    }

    /// Place a block by row name at `place_pos` the way a player click on the
    /// cell below would (the placement ladder: model footprints, orientation,
    /// per-cell state), committed with no body-occupancy check. A preview tool
    /// placing a MODEL block needs exactly this — a raw `set_block_world`
    /// writes one cell with no facing/offset state and renders a fragment.
    /// Returns false when the placement refuses (unknown row, blocked
    /// footprint).
    pub fn place_block(
        world: &mut World,
        name: &str,
        place_pos: [i32; 3],
        player_facing: crate::facing::Facing,
    ) -> bool {
        let Some(id) = super::block::id_by_name(name) else {
            return false;
        };
        let block = crate::block::Block::from_id(id);
        let p = crate::mathh::IVec3::new(place_pos[0], place_pos[1], place_pos[2]);
        let inputs = crate::world::placement::PlaceInputs {
            hit: p - crate::mathh::IVec3::Y,
            normal: crate::mathh::IVec3::Y,
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

    /// Set a placed model block's per-instance parts mask (which optional
    /// `parts` cubes draw), e.g. the forge furnace's `coals`.
    pub fn set_model_parts(world: &mut World, pos: [i32; 3], parts: u32) -> bool {
        world.set_model_parts(
            crate::mathh::IVec3::new(pos[0], pos[1], pos[2]),
            parts,
            None,
        )
    }
}


/// Loading the installed mod packs so a dev tool generates the SAME world the
/// game does.
///
/// Mod worldgen hooks are installed by `ModHost::initialize`, whose only other
/// caller is the game session. Without this a headless tool silently generates
/// a world with every pack's DATA applied but none of its worldgen CODE run —
/// which looks convincing and is wrong, the worst failure mode a preview tool
/// can have.
pub mod mods {
    /// A loaded mod set with its worldgen hooks installed process-wide.
    ///
    /// Hold it for as long as you generate: dropping it releases the mod
    /// instances the installed hooks borrow.
    pub struct WorldgenMods {
        _host: crate::modding::ModHost,
    }

    /// Load every enabled pack's wasm for `seed` and install its worldgen
    /// hooks.
    ///
    /// The init call wants a full simulation context, so this builds a
    /// THROWAWAY one — a scratch world, a player at the origin, an empty GUI
    /// map and bus. Only registrations survive the call; the scratch state is
    /// dropped immediately.
    pub fn load(seed: u32) -> WorldgenMods {
        let mut host = crate::modding::ModHost::load(seed, &Default::default());
        let mut world = crate::world::World::new(seed, 4);
        let mut player = crate::player::Player::new(glam::Vec3::new(0.0, 80.0, 0.0));
        let mut gui = crate::gui_state::empty_gui_state();
        let mut bus = crate::events::EventBus::default();
        let mut systems = crate::events::TickSystems::default();
        let mut sound = 1u64;
        host.initialize(
            &mut world,
            &mut player,
            &mut gui,
            &mut bus,
            &mut systems,
            &mut sound,
        );
        WorldgenMods { _host: host }
    }
}

/// Recipe-catalog lookups for pack checks.
pub mod recipes_query {
    /// The `class` route for `item_key`, as the resulting item key — the same
    /// answer a machine gets from `HostCall::RecipeResult`. `None` when the
    /// item is unknown or the route does not exist.
    pub fn process(
        catalog: &crate::crafting::Recipes,
        class: &str,
        item_key: &str,
    ) -> Option<String> {
        let item = crate::item::ItemType::by_key(item_key)?;
        catalog
            .process(class, item)
            .map(|s| s.item.key().to_owned())
    }
}

/// Mod-driven per-block draw sets — the primitive a mod uses to draw what it
/// SIMULATES. Exposed so a preview tool can submit the same geometry a mod
/// would.
pub mod draw {
    pub use mod_api::DrawPrim;
}

/// The GUI documents every enabled pack ships, for tools that check a pack
/// loaded cleanly.
pub mod gui {
    /// Every accepted `*.gui.json` document as `(kind key, container slot
    /// count)`. A pack machine missing from this list, or listed with zero
    /// container slots, has a REJECTED or mis-declared document — which
    /// otherwise degrades silently to plain storage.
    pub fn loaded_documents() -> Vec<(&'static str, usize)> {
        crate::gui::documents::loaded_documents()
    }
}

/// The loaded recipe catalog and the progression rule derived from it — what
/// a developer tool needs to audit "which recipes does the player see once
/// they have held X", without opening a world.
pub mod recipes {
    pub use crate::crafting::{CraftingCatalog, CraftingRecipe, Recipes, UnlockIndex};

    /// Every enabled pack's crafting/processing rows (the same load the
    /// server runs at session start, with nothing disabled).
    pub fn load() -> Recipes {
        crate::crafting::load_recipes_for(&Default::default())
    }

    /// The recipes a player who has held exactly `item_names` would have
    /// unlocked under the engine's default rule. Unknown names are ignored.
    pub fn opened_by_items(index: &UnlockIndex, item_names: &[&str]) -> Vec<String> {
        let obtained: crate::item::ItemSet = item_names
            .iter()
            .filter_map(|name| crate::item::ItemType::by_name(name))
            .collect();
        index.opened_by_all(&obtained).map(str::to_owned).collect()
    }
}

pub mod biome {
    pub use crate::biome::Biome;
}

pub mod block {
    pub use crate::block::Block;

    /// Runtime id of a namespaced block row key (`"petramond:torch"`), or
    /// `None` when no loaded catalog layer declares it. Ids shift as packs
    /// change, so tools must resolve by name rather than hardcode numbers.
    pub fn id_by_name(name: &str) -> Option<u16> {
        crate::registry::names().blocks.id(name)
    }

    /// The row key a runtime block id came from.
    pub fn name_of(id: u16) -> Option<&'static str> {
        crate::registry::names().blocks.name(id)
    }
}

// Tile colour data, re-exported so dev tools (genmap) can derive block map
// colours from the block rows' top tiles instead of a hand-maintained palette.
pub mod atlas {
    pub use crate::tile::{Tile, TileTint};
}

pub mod chunk {
    pub use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
}

pub mod worldgen {
    //! Worldgen dev-tool surface — moved to `petramond_worldgen::preview`;
    //! this shim keeps the tooling paths stable for genmap/littercensus.
    pub use petramond_worldgen::preview::*;
}
