//! `ChunkGenerator` — owns the worldgen subsystems and runs the fixed stage
//! order for one chunk.
//!
//! Strata P2 stages: Setup → BiomeAssign → (TerrainFill + Carve +
//! SurfaceComposite, fused into one column cascade for byte-parity with the god
//! file's `build_column`). Feature placement is still done by the legacy placer
//! in `worldgen::generate_chunk` until P3 folds it in as the final stage.
//!
//! The generator holds only immutable wiring built from `seed` (no interior
//! mutability) and is `Send + Sync`; its only mutable scratch — a `ColumnGrid`
//! — is a stack-local in `generate`. Output is therefore a pure function of
//! `(seed, cx, cz)`, independent of thread or call order.

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};

use super::carve::CarverSet;
use super::climate::source::{BiomeSource, CASCADE};
use super::ctx::ColumnGrid;
use super::noise::HeightField;
use super::proto::ProtoChunk;
use super::surface::SurfaceSystem;

pub struct ChunkGenerator {
    /// Retained for the feature stage seeding folded in at P3.
    #[allow(dead_code)]
    seed: u32,
    field: HeightField,
    biome_source: &'static dyn BiomeSource,
    surface: SurfaceSystem,
    carvers: CarverSet,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            field: HeightField::new(seed),
            biome_source: &CASCADE,
            surface: SurfaceSystem,
            carvers: CarverSet::default(),
        }
    }

    /// Run terrain generation (everything except features) for one chunk.
    pub fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        let mut grid = ColumnGrid::default();
        self.biome_assign(&mut proto, &mut grid);
        self.fill_columns(&mut proto, &grid);
        proto.into_chunk()
    }

    /// BiomeAssign: sample height/climate/biome/river per column, memoize into
    /// the grid, and write the per-column biome id into the proto.
    fn biome_assign(&self, proto: &mut ProtoChunk, grid: &mut ColumnGrid) {
        let (ox, oz) = proto.chunk_origin_world();
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let surf = self.field.surface_height(wx, wz);
                let climate = self.field.climate(wx, wz);
                let biome = self.biome_source.pick(&climate, surf);
                let river = self.field.river_strength(wx, wz);
                let i = z * CHUNK_SX + x;
                grid.surf[i] = surf;
                grid.biome[i] = biome;
                grid.river[i] = river;
                proto.set_biome(x, z, biome.id());
            }
        }
    }

    /// Fused TerrainFill + Carve + SurfaceComposite. This is the god file's
    /// `build_column` inner cascade verbatim, with the material/carve decisions
    /// delegated to `SurfaceSystem`/`CarverSet`. Branch order is load-bearing
    /// for parity: above-surface water, then river channel, then river bed,
    /// then surface top, then subsurface band, then stone core.
    fn fill_columns(&self, proto: &mut ProtoChunk, grid: &ColumnGrid) {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let i = z * CHUNK_SX + x;
                let surf = grid.surf[i];
                let biome = grid.biome[i];
                let river = grid.river[i];

                let top = self.surface.top_block(biome, surf, river);
                let sub = self.surface.subsurface(biome);
                let plan = self.carvers.plan(river, surf);
                let carve = plan.carve;
                let river_bed_y = plan.river_bed_y;

                for y in 0..CHUNK_SY {
                    let y = y as i32;
                    let b = if y > surf {
                        if y <= SEA_LEVEL { Block::Water } else { Block::Air }
                    } else if carve && y >= river_bed_y && y <= SEA_LEVEL {
                        Block::Water
                    } else if carve && y == river_bed_y - 1 {
                        sub
                    } else if y == surf {
                        top
                    } else if y > surf - 5 {
                        sub
                    } else {
                        Block::Stone
                    };
                    if b != Block::Air {
                        proto.set_block_raw(x, y as usize, z, b.id());
                    }
                }
            }
        }
    }
}
