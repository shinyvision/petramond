//! `ChunkGenerator` — owns the worldgen subsystems and runs the fixed stage
//! order for one chunk.
//!
//! Stages: Setup → BiomeAssign (height/climate/biome/river + per-column overhang
//! plan) → FillColumns (3-D solidity bitmap + top-down skin pass) → Features.
//!
//! The generator holds only immutable wiring built from `seed` (no interior
//! mutability) and is `Send + Sync`; its only mutable scratch — a `ColumnGrid`
//! — is a stack-local in `generate`. Output is therefore a pure function of
//! `(seed, cx, cz)`, independent of thread or call order.

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::mathh::smoothstep;

use super::carve::CarverSet;
use super::climate::source::{BiomeSource, CASCADE};
use super::ctx::ColumnGrid;
use super::data::biomes::def;
use super::field_cache::FieldCache;
use super::noise::HeightField;
use super::proto::ProtoChunk;
use super::surface::rule::SurfaceCtx;
use super::surface::SurfaceSystem;

pub struct ChunkGenerator {
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

    /// Build a per-chunk field cache bound to this generator's height field.
    /// Thread the SAME cache through `generate` then `place_features` so the
    /// per-column field samples are computed once and reused across both stages.
    pub fn field_cache(&self, cx: i32, cz: i32) -> FieldCache<'_> {
        FieldCache::new(&self.field, cx, cz)
    }

    /// Run terrain generation (everything except features) for one chunk.
    pub fn generate(&self, cx: i32, cz: i32, cache: &mut FieldCache) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        let mut grid = ColumnGrid::default();
        self.biome_assign(&mut proto, &mut grid, cache);
        self.fill_columns(&mut proto, &grid, cache);
        proto.into_chunk()
    }

    /// Feature placement stage. Reuses this generator's height field + biome
    /// source + seed so nothing is rebuilt. (P4: world-positional, cross-chunk.)
    pub fn place_features(&self, chunk: &mut Chunk, cache: &mut FieldCache) {
        super::feature::place_features(chunk, cache, &self.carvers, self.biome_source, self.seed);
    }

    /// BiomeAssign: sample height/climate/biome/river per column, memoize into the
    /// grid, write the per-column biome id, and precompute the overhang carve plan
    /// (amplitude + Y band) so the fill stage can skip the 3-D work in flatland.
    fn biome_assign(&self, proto: &mut ProtoChunk, grid: &mut ColumnGrid, cache: &mut FieldCache) {
        let (ox, oz) = proto.chunk_origin_world();
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let surf = cache.surf(wx, wz);
                let climate = cache.climate(wx, wz);
                let biome = self.biome_source.pick(&climate, surf);
                let river = cache.river(wx, wz);
                let i = z * CHUNK_SX + x;
                grid.surf[i] = surf;
                grid.biome[i] = biome;
                grid.river[i] = river;
                proto.set_biome(x, z, biome.id());

                // Overhang plan: mountains get a 3-D carve. Gated on the actual
                // surface height (robust regardless of the noise distribution) so
                // tall peaks always distort; roughness just adds extra amplitude.
                // Onset stays at y96 (just above the y95 treeline) so trees are
                // never anchored on a 3-D-carved column. Amplitude kept at the
                // value that holds the 0-detached-debris invariant (flood audit);
                // the jagged heightfield already supplies the dramatic relief.
                let high = smoothstep(96.0, 128.0, surf as f32) as f64; // 0 below y96
                let er01 = (climate.erosion * 0.5 + 0.5).clamp(0.0, 1.0) as f64;
                let rough = 1.0 - er01;
                let amp = (12.0 * high * (0.5 + 0.5 * rough)) as f32;
                grid.overhang_amp[i] = amp;
                if amp > 0.0 {
                    let a = amp.ceil() as i32;
                    grid.band_lo[i] = (surf - a - 2).max(0);
                    grid.band_hi[i] = (surf + a + 2).min(CHUNK_SY as i32 - 1);
                }
            }
        }
    }

    /// FillColumns: per column build a 3-D solidity bitmap (heightfield in flat
    /// terrain; a band-limited warped surface in mountains, giving overhangs while
    /// a hard anchor below the band makes floating debris impossible), then a
    /// top-down skin pass that fills water and resolves the surface material by
    /// contiguous depth-from-top.
    fn fill_columns(&self, proto: &mut ProtoChunk, grid: &ColumnGrid, cache: &mut FieldCache) {
        let (ox, oz) = proto.chunk_origin_world();
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let i = z * CHUNK_SX + x;
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let surf = grid.surf[i];
                let biome = grid.biome[i];
                let river = grid.river[i];
                let amp = grid.overhang_amp[i];
                let band_lo = grid.band_lo[i];
                let band_hi = grid.band_hi[i];
                let plan = self.carvers.smoothed_plan(cache, wx, wz, river, surf);
                // Biome surface rule looked up ONCE per column (not per voxel).
                let surface_rule = def(biome).surface;

                // --- phase 1: solidity bitmap (stack array, no alloc) ---
                let mut solid = [false; CHUNK_SY];
                for y in 0..CHUNK_SY as i32 {
                    let s = if amp == 0.0 {
                        y <= surf // smooth path: pure heightfield
                    } else if y < band_lo {
                        true // hard anchor -> nothing can float
                    } else if y > band_hi {
                        false
                    } else {
                        // One bare 3-D sample. Carve-ONLY (`n.min(0)`): the warped
                        // surface may undercut BELOW `surf` (cliffs/overhangs) but
                        // never rises above it, so the jagged heightfield can't grow
                        // detached pinnacles — preserving the 0-floating-debris
                        // invariant that the symmetric carve broke on steep peaks.
                        let n = self.field.overhang_noise(wx, y, wz);
                        (y as f64) <= (surf as f64) + (amp as f64) * n.min(0.0)
                    };
                    solid[y as usize] = s;
                }

                // --- phase 2: water + top-down skin pass ---
                let mut depth: u32 = 0;
                for y in (0..CHUNK_SY as i32).rev() {
                    let yi = y as usize;
                    if !solid[yi] {
                        depth = 0;
                        if y <= SEA_LEVEL {
                            proto.set_block_raw(x, yi, z, Block::Water.id());
                        }
                        continue;
                    }
                    // River valley: cut everything above the (strength-sloped)
                    // valley floor, flooding to sea level (air above sea on the dry
                    // upper banks). Smooth, non-mountain columns only. The floor
                    // voxel itself falls through to the skin pass below and becomes
                    // the riverbed (sand near sea via the surface river rule).
                    if amp == 0.0 && plan.carve && y > plan.river_floor {
                        if y <= SEA_LEVEL {
                            proto.set_block_raw(x, yi, z, Block::Water.id());
                        }
                        depth = 0;
                        continue;
                    }
                    let ctx = SurfaceCtx {
                        y,
                        surf_y: surf,
                        depth_from_top: depth,
                        biome,
                        river,
                    };
                    let b = self.surface.skin_block(&ctx, surface_rule);
                    proto.set_block_raw(x, yi, z, b.id());
                    depth += 1;
                }
            }
        }
    }
}
