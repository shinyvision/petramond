//! Explicit river network and column carver.
//!
//! This replaces the classic biome-layer river overlay for active generation.
//! Rivers are generated as deterministic path objects first, then columns query
//! distance to those paths for a valley cross-section carve.
//!
//! Determinism / seam contract: every routing + carve result is a pure function
//! of `(seed, world_x, world_z)`. `path_from_cell` depends only on `(seed, cell)`
//! plus the owned [`land_voronoi`] elevation sampler (itself pure per world pos).
//! `carve_column` reads ONLY its own column's `base_surf`/`biome` plus the global
//! paths — never a neighbour `region.surf`/`region.biome_ids` index — so a column
//! carves identically regardless of which region requests it. The carve is
//! carve-only: it never raises terrain (keeps the floating-debris audit at 0).
//!
//! Module layout (pure code-split; one `RiverSystem` type, concerns separated):
//! - [`source`] — source gating + spacing (`source_gate`/`source_suppressed`).
//! - [`route`] — path routing (path types, the source-cell trace, `flow_dir`,
//!   channel width/depth, terminal pond, region path enumeration).
//! - [`carve`] — the cross-section carve + the two-nearest path query.
//! - [`super::data::rivers`] — the biome→block material tables.

mod carve;
mod route;
mod source;

use std::collections::HashMap;
use std::sync::RwLock;

use noise::{Fbm, MultiFractal, OpenSimplex};

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use crate::mathh::{lerp, smoothstep, smoothstep01};

use super::classic::biome::layers::Layer;
use super::classic::biome::stack::land_voronoi;
use super::classic::world::{map_biome, RegionCells};
use super::data;
use super::rng::FeatureRng;

const SOURCE_SALT: u64 = 0x0000_5249_5645_5253;
const CELL_BLOCKS: i32 = 640;

// --- Routing budget (decisions §3) ---
const PATH_STEPS: usize = 80;
const STEP_BLOCKS: f32 = 26.0;
/// Furthest a query column can be from a path centerline and still be carved.
const MAX_QUERY_RADIUS: f32 = 96.0;
/// Cell-cull reach: how far outside the region we enumerate source cells. A cell
/// is enumerated iff its index is within `PATH_REACH` of the region, so this MUST
/// cover the worst-case distance from a cell's ORIGIN to its farthest carved
/// column = (max source offset inside the cell) + (max path extent
/// `PATH_STEPS*STEP_BLOCKS`) + (query radius `MAX_QUERY_RADIUS`). The decisions
/// doc's `2080 + 96` omits the in-cell source offset; we add a full `CELL_BLOCKS`
/// for it so a far river whose source sits near its cell's far edge is never
/// culled (which would seam). `2080 + 96 + 640 = 2816`.
const PATH_REACH: i32 = 2_816;
/// How far past the first ocean hit the mouth is extended into the sea.
const OCEAN_OVERSHOOT_STEPS: usize = 2;

// --- Source gating + spacing (decisions §4; reworked after first renders) ---
/// Baseline per-cell source probability (seed-uniform density), modulated by the
/// low-freq `source` field for regional clustering. Using that field as a HARD
/// threshold made coverage swing 0%..28% by seed (its DC offset is seed-dependent
/// — some seeds sit mostly above, some mostly below). A per-cell roll fixes that.
const SOURCE_PROB: f32 = 0.45;
const SOURCE_CLUSTER: f32 = 0.08;
const SOURCE_MIN_ELEV: f32 = 64.0;
const SOURCE_MAX_ELEV: f32 = 82.0;
const MIN_SOURCE_SPACING: f32 = 360.0;

// --- Width / depth band (decisions §6) ---
const WET_MIN: f32 = 5.0;
const WET_MAX: f32 = 20.0;
const WET_HEADWATER: f32 = 3.0;
const BED_MIN_DEPTH: f32 = 2.0;
const BED_MAX_DEPTH: f32 = 7.0;

// --- Carve cross-section (decisions §5) ---
const FLOODPLAIN_RISE: f32 = 2.0;
const FLOODPLAIN_FRAC: f32 = 0.9;
/// Amplitude of the floodplain-floor undulation (breaks the flat SEA-level ring).
const FLOODPLAIN_AMP: f32 = 1.6;
const WALL_MIN_RUN: f32 = 8.0;
const WALL_RELIEF_K: f32 = 0.55;
const WALL_RUN_MAX: f32 = 60.0;
const INFLUENCE_CAP: f32 = 95.0;
const EDGE_AMP: f32 = 2.5;

// --- Routing weights / meander (turn-rate limited; no knots) ---
const GRAD_OFFS: f32 = 56.0;
const W_DOWN: f32 = 1.0;
const W_TILT: f32 = 0.8;
const W_MEANDER: f32 = 0.95;
/// Max heading change per step (radians, ~18°). Bounds curvature so a river can
/// never reverse or knot — it only bends gently toward its seaward target. This
/// is the key fix over a raw weighted-sum direction (which could net to a
/// reversed/curling heading and tie the path in knots).
const MAX_TURN: f32 = 0.32;
const MEANDER_BASE: f32 = 260.0;
const MEANDER_K: f32 = 12.0;

const SALT_SOURCE: u32 = 0x5210_0001;
const SALT_WIDTH: u32 = 0x5210_0004;
const SALT_DEPTH: u32 = 0x5210_0005;
const SALT_MATERIAL: u32 = 0x5210_0006;
const SALT_BANK: u32 = 0x5210_0007;

/// River effect at one world column after querying the explicit path network.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RiverColumn {
    /// 0 outside the valley, ramping to 1 over the channel + floodplain.
    pub influence: f32,
    /// 0 outside the wet channel, 1 at the centerline.
    pub channel: f32,
    /// Distance in blocks to the chosen centerline segment.
    pub distance: f32,
    /// Full wet-channel width in blocks at the nearest segment.
    pub width: f32,
    /// River depth in blocks at the nearest segment.
    pub depth: f32,
    /// Top solid river-bed y after carving.
    pub bed_y: i32,
    /// Water surface y for this river column.
    pub water_y: i32,
    /// Block used for exposed river banks and bed, unless an existing water-body
    /// floor should preserve its own material.
    pub bed_block: Block,
    /// Optional exposed bank deposit. `None` means the smoothed bank keeps its
    /// surrounding biome surface, which is common through grass-dominant biomes.
    pub bank_block: Option<Block>,
    /// Existing water-body floors keep their original biome surface rule.
    pub preserve_bed: bool,
}

impl RiverColumn {
    #[inline]
    pub fn active(self) -> bool {
        self.influence > 0.01
    }

    #[inline]
    pub fn wet(self) -> bool {
        is_wet(self.width, self.channel)
    }
}

impl Default for RiverColumn {
    fn default() -> Self {
        Self {
            influence: 0.0,
            channel: 0.0,
            distance: f32::INFINITY,
            width: 0.0,
            depth: 0.0,
            bed_y: SEA_LEVEL - 4,
            water_y: SEA_LEVEL,
            bed_block: Block::Dirt,
            bank_block: None,
            preserve_bed: false,
        }
    }
}

pub struct RiverSystem {
    seed: u32,
    /// Own cheap elevation/ocean sampler (so routing needs no world handle and
    /// `apply`'s signature is unchanged). Pure function of (seed, wx, wz).
    land_voronoi: Box<dyn Layer>,
    source: Fbm<OpenSimplex>,
    width: OpenSimplex,
    depth: OpenSimplex,
    material: OpenSimplex,
    bank: OpenSimplex,
    tilt_x: f32,
    tilt_z: f32,
    /// Precomputed cos/sin of `MAX_TURN` for the per-step turn-rate clamp.
    cos_max_turn: f32,
    sin_max_turn: f32,
    /// Memoized paths per source cell. `compute_path_from_cell` is a pure function
    /// of (seed, cell); caching it makes per-chunk cost negligible after warmup,
    /// because adjacent chunks re-query ~the same nearby cells (feature margin) and
    /// one generator is shared across a whole worldgen run. The `RwLock` keeps
    /// `RiverSystem` `Send + Sync`; a double-compute on a race is harmless (the
    /// function is pure → identical result), so determinism is unaffected.
    path_cache: RwLock<HashMap<(i32, i32), Option<route::RiverPath>>>,
}

impl RiverSystem {
    pub fn new(seed: u32) -> Self {
        // Seam safety: the cell-cull reach must cover the worst-case distance from
        // a cell origin to its farthest carved column — the in-cell source offset
        // (bounded by CELL_BLOCKS), the path extent, and the query radius.
        debug_assert!(
            PATH_REACH as f32
                >= CELL_BLOCKS as f32 + PATH_STEPS as f32 * STEP_BLOCKS + MAX_QUERY_RADIUS,
            "PATH_REACH must cover source offset + max path extent + query radius"
        );
        // ±1-neighbour suppression window sufficiency: sources sit in [0.20,0.80]
        // of a cell, so two sources two cells apart are >= 1.40*CELL_BLOCKS apart.
        // Keeping spacing below that means any pair closer than MIN_SOURCE_SPACING
        // is necessarily in adjacent (±1) cells, so the window can't miss one.
        const _: () = assert!(MIN_SOURCE_SPACING < 1.40 * CELL_BLOCKS as f32);
        const _: () = assert!(INFLUENCE_CAP < MAX_QUERY_RADIUS);

        let mut rng = FeatureRng::positional(seed, SOURCE_SALT, 0, 0, 0);
        let angle = rng.next_f32() * std::f32::consts::TAU;
        Self {
            seed,
            land_voronoi: land_voronoi(seed as i64),
            source: Fbm::<OpenSimplex>::new(seed.wrapping_add(SALT_SOURCE))
                .set_octaves(2)
                .set_frequency(0.0017),
            width: OpenSimplex::new(seed.wrapping_add(SALT_WIDTH)),
            depth: OpenSimplex::new(seed.wrapping_add(SALT_DEPTH)),
            material: OpenSimplex::new(seed.wrapping_add(SALT_MATERIAL)),
            bank: OpenSimplex::new(seed.wrapping_add(SALT_BANK)),
            tilt_x: angle.cos(),
            tilt_z: angle.sin(),
            cos_max_turn: MAX_TURN.cos(),
            sin_max_turn: MAX_TURN.sin(),
            path_cache: RwLock::new(HashMap::new()),
        }
    }

    // --- Elevation sampler (decisions §1) -----------------------------------

    /// Approximate terrain Y from the biome base-height table. Ocean biomes read
    /// low, mountains high. Pure function of (seed, wx, wz).
    fn coarse_elevation(&self, wx: i32, wz: i32) -> f32 {
        let id = self.land_voronoi.gen(wx as i64, wz as i64, 1, 1)[0];
        // 63.75 is the terrain DENSITY sea datum (classic::terrain's private
        // SEA_LEVEL=63 world), NOT chunk::SEA_LEVEL(64) — used only for RELATIVE
        // routing/gating (gradients, the source elevation band), never mixed into
        // the 64-based carve. Do not "fix" it to 64. `biome_height` folds +128.
        63.75 + 17.0 * super::classic::terrain::biome_height(id).0
    }

    /// Whether the column sits on an ocean biome (ocean | frozen_ocean | deep).
    fn is_ocean_at(&self, wx: i32, wz: i32) -> bool {
        let id = self.land_voronoi.gen(wx as i64, wz as i64, 1, 1)[0];
        let base = if id >= 128 { id - 128 } else { id };
        matches!(base, 0 | 10 | 24)
    }

    /// Apply river carving in-place to a base land region.
    pub fn apply(&self, region: &mut RegionCells) {
        let paths = self.paths_for_bounds(
            region.x0,
            region.z0,
            region.x0 + region.w as i32,
            region.z0 + region.h as i32,
        );
        region.rivers.fill(RiverColumn::default());
        if paths.is_empty() {
            return;
        }

        for z in 0..region.h {
            for x in 0..region.w {
                let i = z * region.w + x;
                let wx = region.x0 + x as i32;
                let wz = region.z0 + z as i32;
                let base_surf = region.surf[i];
                let biome = map_biome(region.biome_ids[i]);
                let Some((river, carved_surf)) =
                    self.carve_column(wx, wz, base_surf, biome, &paths)
                else {
                    continue;
                };

                region.rivers[i] = river;
                region.surf[i] = carved_surf;
                if river.wet() && !river.preserve_bed {
                    region.biome_ids[i] = 7; // classic river id, mapped by `map_biome`.
                }
            }
        }
    }
}

#[inline]
fn normalize(v: (f32, f32)) -> Option<(f32, f32)> {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len > 1e-5 {
        Some((v.0 / len, v.1 / len))
    } else {
        None
    }
}

/// Single source of truth for "is this a wet river column". Used by both
/// [`RiverColumn::wet`] and the carve's flood guard so they can never drift.
#[inline]
fn is_wet(width: f32, channel: f32) -> bool {
    width >= WET_MIN && channel > 0.05
}

#[cfg(test)]
mod tests {
    use super::route::{RiverPath, RiverPoint};
    use super::*;
    use crate::worldgen::classic::world::CascadeWorld;

    #[test]
    fn river_system_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RiverSystem>();
    }

    /// A populated sample region for seed 12345 (the origin region has no river
    /// under the tighter source gating; this 2048² block around origin does).
    fn sample_region() -> (CascadeWorld, RegionCells, RiverSystem) {
        let world = CascadeWorld::new(12_345);
        let rivers = RiverSystem::new(12_345);
        let mut region = world.region(-1024, -1024, 2048, 2048);
        rivers.apply(&mut region);
        (world, region, rivers)
    }

    /// A synthetic straight path with the given uniform width/depth, bbox padded.
    pub(super) fn straight_path(width: f32, depth: f32) -> RiverPath {
        RiverPath {
            key: (0, 0),
            points: vec![
                RiverPoint {
                    x: -384.0,
                    z: 0.0,
                    width,
                    depth,
                },
                RiverPoint {
                    x: 384.0,
                    z: 0.0,
                    width,
                    depth,
                },
            ],
            min_x: -384.0 - MAX_QUERY_RADIUS,
            min_z: -MAX_QUERY_RADIUS,
            max_x: 384.0 + MAX_QUERY_RADIUS,
            max_z: MAX_QUERY_RADIUS,
        }
    }

    #[test]
    fn generated_rivers_use_one_fixed_water_level() {
        let (_world, region, _rivers) = sample_region();

        let active: Vec<_> = region.rivers.iter().filter(|r| r.wet()).collect();
        assert!(!active.is_empty(), "sample region should contain a river");
        assert!(
            active.iter().all(|r| r.water_y == SEA_LEVEL),
            "river water level must not vary by column"
        );

        let (min_w, max_w) = active.iter().fold((f32::MAX, f32::MIN), |(lo, hi), r| {
            (lo.min(r.width), hi.max(r.width))
        });
        let (min_d, max_d) = active.iter().fold((f32::MAX, f32::MIN), |(lo, hi), r| {
            (lo.min(r.depth), hi.max(r.depth))
        });
        assert!(
            max_w - min_w > 4.0,
            "river width should vary gradually along the path"
        );
        assert!(
            max_d - min_d >= 2.0,
            "river depth should vary gradually along the path"
        );
    }

    #[test]
    fn no_flat_sea_level_ring() {
        let (_world, region, _rivers) = sample_region();

        let mut bank = 0usize; // just-outside-channel bank columns
        let mut at_sea = 0usize;
        let mut wet_cols = 0usize;
        let mut wet_below_sea = 0usize;
        for (i, river) in region.rivers.iter().enumerate() {
            if river.preserve_bed {
                continue;
            }
            if river.wet() {
                wet_cols += 1;
                if region.surf[i] < SEA_LEVEL {
                    wet_below_sea += 1;
                }
                continue;
            }
            // A just-outside-channel bank column: active, near but past the wet edge.
            if river.active() && river.width >= 8.0 {
                let wet_edge = river.width * 0.5;
                let from_edge = river.distance - wet_edge;
                if (0.0..=2.0).contains(&from_edge) {
                    bank += 1;
                    if region.surf[i] == SEA_LEVEL {
                        at_sea += 1;
                    }
                }
            }
        }

        assert!(bank > 32, "sample should contain measurable river banks");
        assert!(
            (at_sea as f32 / bank as f32) < 0.25,
            "bank columns should not sit at a flat sea-level ring (was {:.2})",
            at_sea as f32 / bank as f32
        );
        assert!(wet_cols > 0, "sample should contain wet columns");
        assert_eq!(
            wet_below_sea, wet_cols,
            "every wet column must carve below sea level — no dry stubs"
        );
    }

    #[test]
    fn carve_is_deterministic_by_world_pos() {
        // THE seam test: the same world column carved via two regions of different
        // origin AND size must produce an identical RiverColumn + carved surf. The
        // sample window lies inside the overlap of both regions and over a river.
        let world = CascadeWorld::new(12_345);
        let rivers = RiverSystem::new(12_345);

        // Region A: x[-256,768) z[-1792,-768). Region B: x[-128,640) z[-1664,-896).
        let mut a = world.region(-256, -1792, 1024, 1024);
        rivers.apply(&mut a);
        let mut b = world.region(-128, -1664, 768, 768);
        rivers.apply(&mut b);

        let mut compared = 0usize;
        let mut active_compared = 0usize;
        let mut wet_compared = 0usize;
        // Sample window must stay inside BOTH regions' overlap: A covers x[-256,768)
        // z[-1792,-768); B covers x[-128,640) z[-1664,-896). Overlap = x[-128,640)
        // z[-1664,-896).
        for wz in (-1640..=-920).step_by(5) {
            for wx in (-120..=620).step_by(5) {
                let ra = a.river_at(wx, wz);
                let rb = b.river_at(wx, wz);
                assert_eq!(ra, rb, "river column differs at ({wx},{wz}) across regions");
                assert_eq!(
                    a.at(wx, wz).0,
                    b.at(wx, wz).0,
                    "carved surf differs at ({wx},{wz}) across regions"
                );
                compared += 1;
                if ra.active() {
                    active_compared += 1;
                }
                if ra.wet() {
                    wet_compared += 1;
                }
            }
        }
        assert!(compared > 0);
        assert!(
            active_compared > 0,
            "seam test should cover some active river columns"
        );
        // The two-nearest median carve is the most order-sensitive path; make sure
        // the seam check actually covered wet channel columns, not just banks.
        assert!(
            wet_compared > 0,
            "seam test should cover some wet river columns"
        );
    }

    #[test]
    fn bedding_material_follows_biome_context() {
        let rivers = RiverSystem::new(12_345);
        let mut desert_sand = 0usize;
        let mut plains_sand = 0usize;
        let mut plains_soil = 0usize;
        let mut mountain_gravel = 0usize;

        for z in 0..48 {
            for x in 0..48 {
                let wx = x * 19 - 380;
                let wz = z * 23 - 540;
                if data::rivers::bed_block(&rivers.material, wx, wz, Biome::Desert) == Block::Sand {
                    desert_sand += 1;
                }
                match data::rivers::bed_block(&rivers.material, wx, wz, Biome::Plains) {
                    Block::Sand => plains_sand += 1,
                    Block::Dirt | Block::CoarseDirt => plains_soil += 1,
                    _ => {}
                }
                if data::rivers::bed_block(&rivers.material, wx, wz, Biome::Mountains)
                    == Block::Gravel
                {
                    mountain_gravel += 1;
                }
            }
        }

        assert!(
            desert_sand > plains_sand * 3,
            "desert/coastal-like contexts should strongly prefer sand bedding"
        );
        assert!(
            plains_soil > plains_sand,
            "grass-dominant contexts should prefer soil bedding over sand"
        );
        assert!(
            mountain_gravel > plains_sand,
            "mountain contexts should expose more gravelly bedding"
        );
    }

    #[test]
    fn grass_biome_exposed_banks_are_sparse_non_dirt_deposits() {
        let (_world, region, _rivers) = sample_region();

        let mut candidates = 0usize;
        let mut deposits = 0usize;
        let mut brown_deposits = 0usize;
        for (i, river) in region.rivers.iter().enumerate() {
            if !river.active() || river.wet() || river.preserve_bed || river.influence < 0.35 {
                continue;
            }
            let biome = map_biome(region.biome_ids[i]);
            if !matches!(
                biome,
                Biome::Plains
                    | Biome::Meadow
                    | Biome::Forest
                    | Biome::BirchForest
                    | Biome::DarkForest
                    | Biome::Jungle
                    | Biome::CherryGrove
                    | Biome::Taiga
                    | Biome::OldGrowthTaiga
            ) {
                continue;
            }

            candidates += 1;
            if let Some(block) = river.bank_block {
                deposits += 1;
                if matches!(block, Block::Dirt | Block::CoarseDirt) {
                    brown_deposits += 1;
                }
            }
        }

        assert!(
            candidates > 64,
            "sample should contain exposed grass-biome river banks"
        );
        assert_eq!(
            brown_deposits, 0,
            "grass-biome exposed bank deposits should not be dirt"
        );
        assert!(
            deposits as f32 / (candidates as f32) < 0.5,
            "most grass-biome banks should keep the biome grass surface"
        );
    }
}
