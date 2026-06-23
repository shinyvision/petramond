//! `ChunkGenerator` — owns the worldgen subsystems and runs the fixed stage
//! order for one chunk.
//!
//! Stages: Setup → RegionBuild (land terrain + explicit rivers) → BiomeAssign
//! → FillColumns (top-down skin pass + caves) → Features.
//!
//! The generator holds only immutable wiring built from `seed` (no interior
//! mutability) and is `Send + Sync`; its only mutable scratch — a `ColumnGrid`
//! — is a stack-local in `generate`. Output is therefore a pure function of
//! `(seed, cx, cz)`, independent of thread or call order.

use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};

use super::classic::terrain::NoiseCache;
use super::classic::world::{map_biome, CascadeWorld, RegionCells};
use super::ctx::ColumnGrid;
use super::data::biomes::def;
use super::noise::settings::{CAVE_MIN_Y, CAVE_SURFACE_BUFFER};
use super::noise::CaveField;
use super::proto::ProtoChunk;
use super::river::RiverSystem;
use super::surface::rule::SurfaceCtx;
use super::surface::SurfaceSystem;

pub struct ChunkGenerator {
    seed: u32,
    field: CaveField,
    world: CascadeWorld,
    rivers: RiverSystem,
    surface: SurfaceSystem,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            field: CaveField::new(seed),
            world: CascadeWorld::new(seed),
            rivers: RiverSystem::new(seed),
            surface: SurfaceSystem,
        }
    }

    /// As [`Self::new`] but the terrain noise is sampled through a shared
    /// [`NoiseCache`]. The worker pool gives every thread's generator the same
    /// cache so overlapping chunk regions sample each lattice column once.
    pub fn with_cache(seed: u32, cache: Arc<NoiseCache>) -> Self {
        Self {
            seed,
            field: CaveField::new(seed),
            world: CascadeWorld::with_cache(seed, cache),
            rivers: RiverSystem::new(seed),
            surface: SurfaceSystem,
        }
    }

    /// The biome-driven world provider (cascade biomes + biome-shaped terrain).
    pub fn world(&self) -> &CascadeWorld {
        &self.world
    }

    /// Compute the region for one chunk PLUS the feature margin in a single pass.
    /// Shared by terrain fill and feature placement, so terrain height, biomes,
    /// and explicit river metadata are generated exactly once.
    pub fn region(&self, cx: i32, cz: i32) -> RegionCells {
        let mut region = super::feature::feature_region(&self.world, cx * 16, cz * 16);
        self.rivers.apply(&mut region);
        region
    }

    /// Run terrain generation (everything except features) for one chunk, reading
    /// the precomputed region.
    pub fn generate(&self, region: &RegionCells, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        let mut grid = ColumnGrid::default();
        self.biome_assign(&mut proto, &mut grid, region);
        self.fill_columns(&mut proto, &grid);
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
    /// right height. (P4: world-positional, cross-chunk.)
    pub fn place_features(&self, chunk: &mut Chunk, region: &RegionCells) {
        super::feature::place_features(chunk, region, self.seed);
    }

    /// BiomeAssign: take each column's biome + biome-driven surface from the shared
    /// region and write it into the grid + the proto's biome ids.
    fn biome_assign(&self, proto: &mut ProtoChunk, grid: &mut ColumnGrid, region: &RegionCells) {
        let (ox, oz) = proto.chunk_origin_world();
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let i = z * CHUNK_SX + x;
                let (surf, biome_id) = region.at(ox + x as i32, oz + z as i32);
                let river = region.river_at(ox + x as i32, oz + z as i32);
                let biome = map_biome(biome_id);
                grid.surf[i] = surf;
                grid.biome[i] = biome;
                grid.river[i] = river;
                proto.set_biome(x, z, biome.id());
            }
        }
    }

    /// FillColumns: per column lay solid terrain up to the final surface, flood
    /// oceans and wet river channels to the fixed water level, resolve the
    /// surface skin by depth-from-top, then carve caves.
    fn fill_columns(&self, proto: &mut ProtoChunk, grid: &ColumnGrid) {
        let (ox, oz) = proto.chunk_origin_world();
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let i = z * CHUNK_SX + x;
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let surf = grid.surf[i];
                let biome = grid.biome[i];
                let river = grid.river[i];
                // Biome surface rule looked up ONCE per column (not per voxel).
                let surface_rule = def(biome).surface;

                // --- phase 1: water + top-down skin pass over a pure heightfield ---
                let mut depth: u32 = 0;
                for y in (0..CHUNK_SY as i32).rev() {
                    let yi = y as usize;
                    if y > surf {
                        depth = 0;
                        if y <= SEA_LEVEL || (river.wet() && y <= river.water_y) {
                            proto.set_block_raw(x, yi, z, Block::Water.id());
                        }
                        continue;
                    }
                    let ctx = SurfaceCtx {
                        y,
                        surf_y: surf,
                        depth_from_top: depth,
                        biome,
                        river: river.influence,
                        water_y: river.water_y,
                        river_bed: river.bed_block,
                        river_bank: river.bank_block,
                        preserve_river_bed: river.preserve_bed,
                    };
                    let b = self.surface.skin_block(&ctx, surface_rule);
                    proto.set_block_raw(x, yi, z, b.id());
                    depth += 1;
                }

                // --- phase 3: caves. Punch 3-D tunnels + caverns through the
                // stone, keeping a solid floor (>= CAVE_MIN_Y) and solid rock
                // under the surface skin (< surf - CAVE_SURFACE_BUFFER) so there
                // are no surface holes and the world floor stays intact. Carving
                // happens AFTER the skin pass, so the grass/dirt cap and its depth
                // bands are already resolved — no "floating grass under a cave".
                // Only Stone is carved; the surface skin and water are untouched.
                let cave_top = surf - CAVE_SURFACE_BUFFER;
                let mut cy = CAVE_MIN_Y;
                while cy < cave_top {
                    let cyi = cy as usize;
                    if proto.block_raw(x, cyi, z) == Block::Stone.id()
                        && self.field.cave_carved(wx, cy, wz)
                    {
                        proto.set_block_raw(x, cyi, z, Block::Air.id());
                    }
                    cy += 1;
                }
            }
        }
    }
}
