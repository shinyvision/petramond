pub mod stream {
    pub use crate::world::{MemoryCensus, ServerWorld};
    pub use petramond_math::facing::Facing;

    pub fn tick(world: &mut ServerWorld, n: u32) {
        for _ in 0..n {
            world.game_tick();
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
        world.commit_placement(&plan)
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

pub mod sim {
    use petramond_math::world_pos::WorldPos;
    use petramond_world::chunk::ChunkPos;

    /// The real game without a window: an unsaved listen server with the installed packs and
    /// one local player that a tool holds in place while the world runs around it.
    pub struct Sim {
        server: crate::server::game::ServerGame,
    }

    /// One live mob as a tool sees it.
    #[derive(Clone, Debug)]
    pub struct SimMob {
        pub id: u64,
        pub key: &'static str,
        pub category: Option<&'static str>,
        pub pos: [f64; 3],
        pub vel: [f32; 3],
        /// The cell at the middle of its body holds a fluid.
        pub in_fluid: bool,
        /// Its tags, values written out.
        pub tags: Vec<(String, String)>,
    }

    impl Sim {
        pub fn new(seed: u32, render_dist: i32) -> Sim {
            let local = crate::server::session_build::LocalPlayer {
                key: crate::net::identity::PlayerIdentity::generate()
                    .expect("os randomness")
                    .key(),
                name: "sim".into(),
            };
            let (server, ..) =
                crate::server::session_build::build_server("", seed, render_dist, Some(local))
                    .expect("build simulation session");
            Sim { server }
        }

        /// Runs `ticks` fixed ticks with the player held at `pos`.
        pub fn run(&mut self, pos: [f64; 3], ticks: u32) {
            let at = WorldPos::new(pos[0], pos[1], pos[2]);
            for _ in 0..ticks {
                self.server.hold_local_player(at);
                self.server
                    .pump_tagged(crate::events::tick::TICK_DT, &mut Vec::new(), &[]);
            }
        }

        /// Whether the world around `pos` has streamed in far enough for mobs to spawn there.
        pub fn ready_at(&self, pos: [f64; 3]) -> bool {
            let chunk = ChunkPos::new(
                (pos[0].floor() as i32).div_euclid(16),
                (pos[2].floor() as i32).div_euclid(16),
            );
            self.server.world().mob_census_loaded_around(chunk, 9)
        }

        pub fn tick(&self) -> u64 {
            self.server.world().current_tick()
        }

        /// The highest cell in the column that blocks movement, once its chunk is loaded.
        pub fn floor_y(&self, x: i32, z: i32) -> Option<i32> {
            let data = self.server.world().data();
            data.chunk_loaded(x.div_euclid(16), z.div_euclid(16))
                .then(|| {
                    (-64..=256)
                        .rev()
                        .find(|&y| data.blocks_movement_at(x, y, z))
                })
                .flatten()
        }

        pub fn mobs(&self) -> Vec<SimMob> {
            let world = self.server.world();
            world
                .mobs()
                .instances()
                .iter()
                .filter(|m| !m.is_dead())
                .map(|m| {
                    let def = crate::mob::def(m.kind);
                    let mid = m.pos.y + f64::from(def.size.height) * 0.5;
                    let v = m.vel();
                    SimMob {
                        id: m.id(),
                        key: def.key,
                        category: def.category.map(|c| c.def().name),
                        pos: [m.pos.x, m.pos.y, m.pos.z],
                        vel: [v.x, v.y, v.z],
                        in_fluid: world.data().fluid_cell_at(
                            m.pos.x.floor() as i32,
                            mid.floor() as i32,
                            m.pos.z.floor() as i32,
                        ),
                        tags: m
                            .tags()
                            .iter()
                            .map(|(k, v)| {
                                let v = match v {
                                    crate::mob::MobTagValue::String(s) => s.clone(),
                                    crate::mob::MobTagValue::Bool(b) => b.to_string(),
                                    crate::mob::MobTagValue::Int(i) => i.to_string(),
                                    crate::mob::MobTagValue::Float(f) => f.to_string(),
                                };
                                (k.clone(), v)
                            })
                            .collect(),
                    }
                })
                .collect()
        }
    }
}
