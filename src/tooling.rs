//! Explicit facade for package preview tools.
//!
//! The game/runtime modules stay crate-internal; binaries under `src/bin` are
//! separate crates, so they use this narrow surface.

/// The live `World`, for dev tools that must observe or drive real streaming
/// rather than a re-derivation of it — generation/light/mesh pumping, the
/// resident-memory census, and deterministic ticking.
pub mod stream {
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
}

pub mod scene;

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
        let mut gui = crate::gui::empty_gui_state();
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
    pub fn id_by_name(name: &str) -> Option<u8> {
        crate::registry::names().blocks.id(name)
    }

    /// The row key a runtime block id came from.
    pub fn name_of(id: u8) -> Option<&'static str> {
        crate::registry::names().blocks.name(id)
    }
}

// Tile colour data, re-exported so dev tools (genmap) can derive block map
// colours from the block rows' top tiles instead of a hand-maintained palette.
pub mod atlas {
    pub use crate::atlas::{Tile, TileTint};
}

pub mod chunk {
    pub use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
}

pub mod worldgen {
    use std::collections::HashMap;

    use crate::block::Block;
    use crate::chunk::Chunk;
    use crate::mathh::IVec3;
    use crate::worldgen::feature::{FeatureCtx, VoxelSink};
    use crate::worldgen::rng::FeatureRng;

    const FEATURE_PREVIEW_SALT: u64 = 0x0000_FE47_0000_0001;

    pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
        crate::worldgen::generate_chunk(seed, cx, cz)
    }

    /// The underground biome owning each position — the SAME answer the
    /// carver's lining and caliber read. A per-biome census needs it: judging
    /// "did this row line every floor in its territory" by proximity to the
    /// row's own lining block silently counts the neighbouring biome's rim as
    /// a miss.
    pub fn underground_biome_at(seed: u32, positions: &[[i32; 3]]) -> Vec<u8> {
        crate::worldgen::underground_biomes_at(seed, positions)
    }

    /// The underground-biome id registered under `name`.
    pub fn underground_biome_id(name: &str) -> Option<u8> {
        crate::worldgen::data::underground::id_by_name(name)
    }

    /// Whether the generated terrain is solid at each position — the same
    /// answer a mod's `terrain_solid_at` gets. A pack that mixes this with the
    /// section snapshot (positional for a neighbour in the next section,
    /// `GenCtx::block` for its own cells) is only sound where the two coincide,
    /// and that is a property of the INSTALLED set, not of bare terrain, so it
    /// wants an instrument outside the engine's own parity test.
    pub fn terrain_solid_at(seed: u32, positions: &[[i32; 3]]) -> Vec<bool> {
        crate::worldgen::terrain_solid_at(seed, positions)
    }

    // Cubic per-section generation, re-exported so dev tools (genmap's deep
    // cross-section / cave statistics) can inspect terrain below y = 0 — the
    // whole-column `Chunk` preview only covers `[0, CHUNK_SY)`.
    pub use crate::chunk::{SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE, WORLD_MIN_Y};
    pub use crate::section::Section;
    pub use crate::worldgen::driver::{ChunkGenerator, ColumnGen};

    /// A kilometre-scale surface overview sampled straight from the climate
    /// graph (no chunk generation): per grid point the classified biome id and
    /// the base surface height. `side` points per edge, `stride` blocks apart,
    /// centred on the origin — for verifying world-scale structure (mountain
    /// belts, valley networks) that a chunk-sized genmap window cannot show.
    pub struct MacroSurfaceMap {
        pub side: usize,
        pub biomes: Vec<u8>,
        pub heights: Vec<f64>,
    }

    pub fn macro_surface_map(seed: u32, side: usize, stride: i32) -> MacroSurfaceMap {
        use crate::worldgen::biome::climate::{
            BiomeClimateIndex, ClimateSampleCell, ClimateSampler,
        };
        use crate::worldgen::density::terrain::{channels, TerrainDensitySpec};
        use crate::worldgen::graph::SamplePoint;

        let graph = TerrainDensitySpec::default_surface().build_graph(seed);
        let index = BiomeClimateIndex::default_surface().clone();
        let sampler = ClimateSampler::new(graph.graph());
        let half = (side as i32 / 2) * stride;
        let mut biomes = Vec::with_capacity(side * side);
        let mut heights = Vec::with_capacity(side * side);
        for gz in 0..side as i32 {
            for gx in 0..side as i32 {
                let wx = gx * stride - half;
                let wz = gz * stride - half;
                let biome = sampler
                    .sample_surface_cell(ClimateSampleCell::surface(wx, wz))
                    .and_then(|sample| index.classify_surface(sample.climate))
                    .map(|b| b as u8)
                    .unwrap_or(0);
                let height = graph
                    .graph()
                    .evaluate_channel(
                        channels::BASE_HEIGHT,
                        SamplePoint::new(f64::from(wx), 0.0, f64::from(wz)),
                    )
                    .unwrap_or(0.0);
                biomes.push(biome);
                heights.push(height);
            }
        }
        MacroSurfaceMap {
            side,
            biomes,
            heights,
        }
    }

    pub fn feature_preview_names() -> &'static [&'static str] {
        &[
            "redwood",
            "oak_young",
            "oak_small",
            "oak_swamp",
            "oak_big",
            "spruce",
            "birch",
            "jungle",
            "acacia",
        ]
    }

    pub fn preview_feature(name: &str, seed: u32) -> Option<FeaturePreview> {
        let cf = configured_feature(name)?;
        let mut sink = PreviewSink::default();
        let mut ctx = FeatureCtx::new(&mut sink);
        let mut rng = FeatureRng::positional(seed, FEATURE_PREVIEW_SALT, 0, 0, 0);
        cf.feature.generate(&mut ctx, IVec3::new(0, 0, 0), &mut rng);

        let mut bounds = FeatureBounds::empty();
        let mut voxels: Vec<FeatureVoxel> = sink
            .voxels
            .into_iter()
            .map(|(pos, block)| {
                bounds.include(pos);
                FeatureVoxel {
                    pos: [pos.x, pos.y, pos.z],
                    block,
                }
            })
            .collect();
        voxels.sort_by_key(|v| (v.pos[1], v.pos[2], v.pos[0], v.block.id()));
        Some(FeaturePreview { voxels, bounds })
    }

    fn configured_feature(
        name: &str,
    ) -> Option<&'static crate::worldgen::feature::ConfiguredFeature> {
        let key = name.trim().to_ascii_lowercase().replace('-', "_");
        let key = match key.as_str() {
            "young_oak" => "oak_young",
            "oak" => "oak_small",
            "swamp_oak" => "oak_swamp",
            "giant_oak" | "fancy_oak" => "oak_big",
            other => other,
        };
        crate::worldgen::data::features::by_name(&format!("petramond:{key}"))
    }

    #[derive(Clone, Debug)]
    pub struct FeaturePreview {
        pub voxels: Vec<FeatureVoxel>,
        pub bounds: FeatureBounds,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct FeatureVoxel {
        pub pos: [i32; 3],
        pub block: Block,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct FeatureBounds {
        pub min: [i32; 3],
        pub max: [i32; 3],
        pub empty: bool,
    }

    impl FeatureBounds {
        fn empty() -> Self {
            Self {
                min: [0; 3],
                max: [0; 3],
                empty: true,
            }
        }

        fn include(&mut self, p: IVec3) {
            if self.empty {
                self.min = [p.x, p.y, p.z];
                self.max = [p.x, p.y, p.z];
                self.empty = false;
                return;
            }
            self.min[0] = self.min[0].min(p.x);
            self.min[1] = self.min[1].min(p.y);
            self.min[2] = self.min[2].min(p.z);
            self.max[0] = self.max[0].max(p.x);
            self.max[1] = self.max[1].max(p.y);
            self.max[2] = self.max[2].max(p.z);
        }
    }

    #[derive(Default)]
    struct PreviewSink {
        voxels: HashMap<IVec3, Block>,
    }

    impl VoxelSink for PreviewSink {
        fn get(&self, p: IVec3) -> Block {
            self.voxels.get(&p).copied().unwrap_or(Block::Air)
        }

        fn set(&mut self, p: IVec3, b: Block) {
            self.voxels.insert(p, b);
        }
    }

    pub mod audit {
        pub use crate::worldgen::audit::{
            audit, flood_audit, relief_audit, roughness, BiomeShare, DebrisAudit, FloodAudit,
            HeightStats, ReliefStats, RoughnessStats, RELIEF_HIST_LABELS,
        };
    }
}
