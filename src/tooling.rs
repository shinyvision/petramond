//! Explicit facade for package developer binaries and diagnostics.
//!
//! The game/runtime modules stay crate-internal; binaries under `src/bin` are
//! separate crates, so they use this narrow surface for probes and benchmarks.

pub mod biome {
    pub use crate::biome::Biome;
}

pub mod block {
    pub use crate::block::Block;
}

pub mod block_model {
    pub use crate::block_model::BlockModelKind;

    pub fn cell_offsets(kind: BlockModelKind) -> impl Iterator<Item = [u8; 3]> {
        crate::block_model::instance(kind)
            .cells
            .iter()
            .map(|cell| cell.offset)
    }
}

pub mod chunk {
    pub use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};
}

pub mod furnace {
    pub use crate::furnace::Facing;
}

pub mod mesh {
    pub use crate::mesh::{build_mesh, compute_chunk_skylight_with_neighbors, ChunkMesh};
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

    pub fn feature_preview_names() -> &'static [&'static str] {
        &[
            "redwood",
            "oak_small",
            "oak_swamp",
            "oak_big",
            "spruce",
            "birch",
            "jungle",
            "acacia",
            "dark_oak",
            "cherry",
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
        Some(match key.as_str() {
            "redwood" => &crate::worldgen::data::features::REDWOOD,
            "oak_small" | "oak" => &crate::worldgen::data::features::OAK_SMALL,
            "oak_swamp" | "swamp_oak" => &crate::worldgen::data::features::OAK_SWAMP,
            "oak_big" | "giant_oak" | "fancy_oak" => &crate::worldgen::data::features::OAK_BIG,
            "spruce" => &crate::worldgen::data::features::SPRUCE,
            "birch" => &crate::worldgen::data::features::BIRCH,
            "jungle" => &crate::worldgen::data::features::JUNGLE,
            "acacia" => &crate::worldgen::data::features::ACACIA,
            "dark_oak" => &crate::worldgen::data::features::DARK_OAK,
            "cherry" => &crate::worldgen::data::features::CHERRY,
            _ => return None,
        })
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

    pub fn generate_chunk_with(generator: &ChunkGenerator, cx: i32, cz: i32) -> Chunk {
        crate::worldgen::generate_chunk_with(&generator.inner, cx, cz)
    }

    pub struct ChunkGenerator {
        inner: crate::worldgen::driver::ChunkGenerator,
    }

    impl ChunkGenerator {
        pub fn new(seed: u32) -> Self {
            Self {
                inner: crate::worldgen::driver::ChunkGenerator::new(seed),
            }
        }

        pub fn region(&self, cx: i32, cz: i32) -> Region {
            Region {
                inner: self.inner.region(cx, cz),
            }
        }

        pub fn generate(&self, region: &Region, cx: i32, cz: i32) -> Chunk {
            self.inner.generate(&region.inner, cx, cz)
        }

        pub fn generate_surface(&self, cx: i32, cz: i32) -> Chunk {
            self.inner.generate_surface(cx, cz)
        }

        pub fn place_underground(&self, chunk: &mut Chunk) {
            self.inner.place_underground(chunk);
        }

        pub fn place_vegetation(&self, chunk: &mut Chunk) {
            self.inner.place_vegetation(chunk);
        }

        pub fn place_features(&self, chunk: &mut Chunk, region: &Region) {
            self.inner.place_features(chunk, &region.inner);
        }

        pub fn place_features_runtime(&self, chunk: &mut Chunk) {
            self.inner.place_features_runtime(chunk);
        }

        // --- Per-section (live streaming) path, for profiling ---------------------

        pub fn generate_column_data(&self, cx: i32, cz: i32) -> ColumnData {
            ColumnData {
                inner: self.inner.generate_column_gen(cx, cz),
            }
        }

        /// Generate one section from shared column data (the live path) and return a
        /// cheap sink value so the work can't be optimised away.
        pub fn generate_section(&self, col: &ColumnData, cy: i32) -> u64 {
            let sp = crate::chunk::SectionPos::new(col.inner.cx(), cy, col.inner.cz());
            let s = self.inner.generate_section(sp, &col.inner);
            s.block_raw(0, 0, 0) as u64
        }
    }

    pub struct ColumnData {
        inner: crate::worldgen::driver::ColumnGen,
    }

    pub struct Region {
        inner: crate::worldgen::region::RegionCells,
    }

    pub mod audit {
        pub use crate::worldgen::audit::{
            audit, flood_audit, relief_audit, roughness, BiomeShare, DebrisAudit, FloodAudit,
            HeightStats, ReliefStats, RoughnessStats, RELIEF_HIST_LABELS,
        };
    }
}

/// Thin wrapper over the crate-internal streaming `World`, exposing just the driving
/// surface a profiler binary needs to measure the live generate→mesh→light pipeline.
pub mod stream {
    pub struct World {
        inner: crate::world::World,
    }

    impl World {
        pub fn new(seed: u32, render_dist: i32) -> Self {
            Self {
                inner: crate::world::World::new(seed, render_dist),
            }
        }
        pub fn update_load(&mut self, cx: i32, cy: i32, cz: i32) {
            self.inner.update_load(cx, cy, cz);
        }
        pub fn update_load_facing(
            &mut self,
            cx: i32,
            cy: i32,
            cz: i32,
            forward_x: f32,
            forward_z: f32,
        ) {
            self.inner
                .update_load_facing(cx, cy, cz, forward_x, forward_z);
        }
        pub fn poll(&mut self) -> usize {
            self.inner.poll()
        }
        pub fn tick_mesh_budget(&mut self, n: usize) {
            self.inner.tick_mesh_budget(n);
        }
        pub fn has_dirty_meshes(&self) -> bool {
            self.inner.has_dirty_meshes()
        }
        pub fn loaded_section_count(&self) -> usize {
            self.inner.loaded_section_count()
        }
        pub fn loaded_column_count(&self) -> usize {
            self.inner.loaded_column_count()
        }
        pub fn mesh_count(&self) -> usize {
            self.inner.iter_meshes().count()
        }
        pub fn dirty_mesh_count(&self) -> usize {
            self.inner.dirty_mesh_count()
        }
    }
}
