//! `ChunkGenerator` — owns the worldgen subsystems and runs the fixed stage
//! order for one chunk.
//!
//! Hot stages: Setup → SurfaceDensityFill → Underground → Vegetation → Features.
//! The older full surface region path remains available for diagnostics and
//! tooling that needs a materialized feature/audit window.
//!
//! The generator holds only immutable wiring built from `seed` (no interior
//! mutability). Output is therefore a pure function of `(seed, cx, cz)`,
//! independent of thread or call order.

use crate::chunk::Chunk;

use super::density::surface::SurfaceDensitySystem;
use super::proto::ProtoChunk;
use super::region::RegionCells;

pub struct ChunkGenerator {
    seed: u32,
    surface_density: SurfaceDensitySystem,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            surface_density: SurfaceDensitySystem::new(seed),
        }
    }

    /// Compute the region for one chunk PLUS the feature margin in a single pass.
    /// Shared by terrain fill and feature placement, so terrain height and biomes
    /// are generated exactly once.
    pub fn region(&self, cx: i32, cz: i32) -> RegionCells {
        let (x0, z0, w, h) = super::feature::feature_region_bounds(cx * 16, cz * 16);
        self.surface_density.region(x0, z0, w, h)
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> crate::biome::Biome {
        self.surface_density.biome_at(wx, wz)
    }

    /// Run terrain generation (everything except features) for one chunk, reading
    /// the precomputed region. Kept for staged tooling and diagnostics.
    pub fn generate(&self, region: &RegionCells, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        self.surface_density.fill_chunk(&mut proto, region);
        proto.into_chunk()
    }

    /// Run hot-path terrain generation for one chunk without materializing a
    /// padded feature region.
    pub fn generate_surface(&self, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        self.surface_density.fill_chunk_direct(&mut proto);
        proto.into_chunk()
    }

    /// Underground scatter stage: ore veins + stone / dirt / gravel blobs that
    /// overwrite Stone below the surface. Runs before features (vegetation) and is
    /// a pure function of `(seed, cx, cz)`.
    pub fn place_underground(&self, chunk: &mut Chunk) {
        super::feature::scatter::place_underground(chunk, self.seed);
    }

    /// Ground-vegetation stage: single-block plants (grass, flowers, ferns,
    /// mushrooms, dead bushes) keyed to biome + surface material. Runs after the
    /// underground pass and BEFORE trees so it reads bare ground.
    pub fn place_vegetation(&self, chunk: &mut Chunk) {
        super::feature::vegetation::place_vegetation(chunk, self.seed);
    }

    /// Feature placement stage. Reads biome + biome-driven surface from the shared
    /// region (incl. the cross-chunk margin) so trees land in the right biome at the
    /// right height. Kept for staged tooling and diagnostics.
    pub fn place_features(&self, chunk: &mut Chunk, region: &RegionCells) {
        super::feature::place_features(chunk, region, self.seed);
    }

    /// Hot-path feature placement. Builds only the feature candidate/support
    /// windows needed by tree placement instead of a full surf+biome audit region.
    pub fn place_features_runtime(&self, chunk: &mut Chunk) {
        let (ox, oz) = chunk.chunk_origin_world();
        let mut field = super::feature::RuntimeFeatureField::new(&self.surface_density, ox, oz);
        super::feature::place_features_with_field(chunk, &mut field, self.seed);
    }
}
