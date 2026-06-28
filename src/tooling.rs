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
    use crate::chunk::Chunk;

    pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
        crate::worldgen::generate_chunk(seed, cx, cz)
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

        pub fn place_underground(&self, chunk: &mut Chunk) {
            self.inner.place_underground(chunk);
        }

        pub fn place_vegetation(&self, chunk: &mut Chunk) {
            self.inner.place_vegetation(chunk);
        }

        pub fn place_features(&self, chunk: &mut Chunk, region: &Region) {
            self.inner.place_features(chunk, &region.inner);
        }
    }

    pub struct Region {
        inner: crate::worldgen::classic::world::RegionCells,
    }

    impl Region {
        pub fn river_at(&self, wx: i32, wz: i32) -> RiverColumn {
            let river = self.inner.river_at(wx, wz);
            RiverColumn {
                influence: river.influence,
                channel: river.channel,
                distance: river.distance,
                wet: river.wet(),
            }
        }
    }

    #[derive(Copy, Clone, Debug, PartialEq)]
    pub struct RiverColumn {
        pub influence: f32,
        pub channel: f32,
        pub distance: f32,
        wet: bool,
    }

    impl RiverColumn {
        pub fn wet(self) -> bool {
            self.wet
        }
    }

    pub mod audit {
        pub use crate::worldgen::audit::{
            audit, flood_audit, relief_audit, roughness, BiomeShare, DebrisAudit, FloodAudit,
            HeightStats, ReliefStats, RoughnessStats, SubSeaBand, RELIEF_HIST_LABELS,
        };
    }
}
