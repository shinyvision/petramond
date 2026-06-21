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

use std::collections::HashMap;
use std::sync::RwLock;

use noise::{Fbm, MultiFractal, NoiseFn, OpenSimplex};

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use crate::mathh::{lerp, smoothstep, smoothstep01};

use super::classic::biome::layers::Layer;
use super::classic::biome::stack::land_voronoi;
use super::classic::world::{map_biome, RegionCells};
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
    path_cache: RwLock<HashMap<(i32, i32), Option<RiverPath>>>,
}

/// A source that passed the cheap gate: jittered source point + its score.
#[derive(Copy, Clone)]
struct SourceGate {
    x: f32,
    z: f32,
    score: f32,
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
        debug_assert!(MIN_SOURCE_SPACING < 1.40 * CELL_BLOCKS as f32);
        debug_assert!(INFLUENCE_CAP < MAX_QUERY_RADIUS);

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

    fn carve_column(
        &self,
        wx: i32,
        wz: i32,
        base_surf: i32,
        biome: Biome,
        paths: &[RiverPath],
    ) -> Option<(RiverColumn, i32)> {
        if matches!(biome, Biome::DeepOcean | Biome::MushroomFields) {
            return None;
        }

        let px = wx as f32 + 0.5;
        let pz = wz as f32 + 0.5;
        let (best, second) = nearest_hits(px, pz, paths);
        let hit = best?;

        let sea = SEA_LEVEL as f32;
        let relief = (base_surf - SEA_LEVEL).max(0) as f32;
        let preserve_bed = base_surf <= SEA_LEVEL;
        let steepness = self.bank_steepness(wx, wz, biome, relief);

        // Radii outward from the centerline (decisions §5).
        let wet_half = (hit.width * 0.5).max(0.0);
        let edge = (wet_half + self.edge_noise(wx, wz)).max(0.0);
        let floodband = (WALL_MIN_RUN * 0.5 + wet_half * (FLOODPLAIN_FRAC - 0.5)).max(2.0);
        let flood_out = edge + floodband;
        let wall_run = (WALL_MIN_RUN + relief * WALL_RELIEF_K).clamp(WALL_MIN_RUN, WALL_RUN_MAX);
        let wall_out = flood_out + wall_run;
        let influence_radius = wall_out.min(INFLUENCE_CAP);
        if hit.distance >= influence_radius {
            return None;
        }

        // The shared cross-section profile for one hit at distance `d`. Monotone
        // non-increasing toward the centre, with NO flat sea-level ring. The
        // per-column noises are pure functions of (wx,wz), so the profile stays
        // identical for both hits and across regions.
        let bed_y = (sea - hit.depth).round().clamp(3.0, sea - 1.0);
        let flood_noise = self.floodplain_noise(wx, wz);
        let profile = |d: f32| -> f32 {
            if d <= edge && edge > 0.5 {
                // Wet channel (concave).
                let t = (d / edge).clamp(0.0, 1.0);
                bed_y + smoothstep01(t) * ((sea - 1.0 - bed_y).max(0.0))
            } else if d <= flood_out {
                // Dry, dished floodplain — rises gently from the wet rim to the
                // dry floor level, undulating so it never forms a flat strip at
                // exactly SEA. On flat plains the floor dips toward base-1. The
                // undulation fades to 0 at both joins so the floodplain meets the
                // wet channel and the valley wall seamlessly.
                let u = smoothstep(edge, flood_out, d);
                let fp_inner = sea - 1.0; // continuous with the wet-channel rim
                let fp_outer = (sea + FLOODPLAIN_RISE).min(base_surf as f32 - 1.0);
                let env = smoothstep(0.0, 0.25, u) * (1.0 - smoothstep(0.75, 1.0, u));
                lerp(fp_inner, fp_outer.max(fp_inner), u) + flood_noise * env
            } else {
                // Valley wall: rise from the floodplain floor up to untouched
                // terrain, steeper biomes hugging the floor longer (concave-up).
                let v = smoothstep(flood_out, wall_out, d);
                let wall_exp = 1.0 + steepness * 0.8;
                let lo = (sea + FLOODPLAIN_RISE)
                    .min(base_surf as f32 - 1.0)
                    .max(sea - 1.0);
                let hi = (base_surf as f32).max(lo);
                lo + (hi - lo) * v.powf(wall_exp) + self.rim_noise(wx, wz, relief, steepness, v)
            }
        };

        // Two-nearest carve safety net (decisions §7): the deepest target wins so
        // a near-parallel sibling channel floods the median between them.
        let mut target = profile(hit.distance);
        if let Some(other) = second {
            if other.distance < influence_radius {
                target = target.min(profile(other.distance));
            }
        }

        let mut carved_surf = (target.round() as i32).min(base_surf).max(3);
        debug_assert!(
            carved_surf <= base_surf,
            "carve must never raise terrain (carve-only invariant)"
        );

        // The column is wet if it lies inside the (noisy) wet edge of EITHER hit;
        // `channel` ramps 0→1 from that edge to the nearest wet centerline so that
        // a flooded median between two channels still reads as wet.
        let in_wet_best = hit.distance < edge;
        let in_wet_second = second.is_some_and(|o| o.distance < edge);
        let wet_distance = match (in_wet_best, in_wet_second) {
            (true, true) => hit
                .distance
                .min(second.map_or(hit.distance, |o| o.distance)),
            (true, false) => hit.distance,
            (false, true) => second.map_or(hit.distance, |o| o.distance),
            (false, false) => hit.distance, // outside both; channel will clamp to 0
        };
        let channel = if edge > 0.01 {
            (1.0 - wet_distance / edge).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // A column that reads as wet MUST flood. The carve's wet branch (gated on
        // `edge > 0.5`) and `wet()` (path width + channel) can disagree at a pinch
        // where edge-noise collapses `edge` while `hit.width >= WET_MIN`, leaving a
        // wet column carved up in the floodplain (>= SEA) — a dry stub. Force any
        // wet column below the waterline. Still carve-only (only lowers); pure
        // function of the column, so seam-safe.
        if is_wet(hit.width, channel) {
            carved_surf = carved_surf.min(SEA_LEVEL - 1);
        }
        let influence = 1.0 - smoothstep(flood_out, influence_radius, hit.distance);
        if influence <= 0.01 && !(in_wet_best || in_wet_second) {
            return None;
        }

        Some((
            RiverColumn {
                influence,
                channel,
                distance: hit.distance,
                width: hit.width,
                depth: hit.depth,
                bed_y: bed_y as i32,
                water_y: SEA_LEVEL,
                bed_block: self.bed_block(wx, wz, biome),
                bank_block: self.bank_block(wx, wz, biome, influence, hit.width),
                preserve_bed,
            },
            carved_surf,
        ))
    }

    fn paths_for_bounds(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<RiverPath> {
        let cx0 = (x0 - PATH_REACH).div_euclid(CELL_BLOCKS);
        let cz0 = (z0 - PATH_REACH).div_euclid(CELL_BLOCKS);
        let cx1 = (x1 + PATH_REACH).div_euclid(CELL_BLOCKS);
        let cz1 = (z1 + PATH_REACH).div_euclid(CELL_BLOCKS);
        let mut paths = Vec::new();
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                if let Some(path) = self.path_from_cell(cx, cz) {
                    if path.intersects(x0 as f32, z0 as f32, x1 as f32, z1 as f32) {
                        paths.push(path);
                    }
                }
            }
        }
        paths
    }

    /// Cheap source gate (decisions §4): fbm score short-circuit first, then
    /// non-ocean + elevation band. Returns the jittered source point + score, or
    /// `None`. Pure function of (seed, cell). Neighbours re-run only THIS (cheap),
    /// never the full trace.
    fn source_gate(&self, cx: i32, cz: i32) -> Option<SourceGate> {
        let ox = cx * CELL_BLOCKS;
        let oz = cz * CELL_BLOCKS;
        let center_x = ox + CELL_BLOCKS / 2;
        let center_z = oz + CELL_BLOCKS / 2;
        let mut rng = FeatureRng::positional(self.seed, SOURCE_SALT, cx, 0, cz);
        // Seed-uniform density: a per-cell roll gated by a probability, with the
        // low-freq `source` field only MODULATING density for regional clustering.
        let roll = rng.next_f32();
        let jx = rng.next_f32();
        let jz = rng.next_f32();
        let cluster = self.source.get([center_x as f64, center_z as f64]) as f32;
        let prob = (SOURCE_PROB + SOURCE_CLUSTER * cluster).clamp(0.05, 0.95);
        if roll > prob {
            return None;
        }

        let x = ox as f32 + CELL_BLOCKS as f32 * (0.20 + 0.60 * jx);
        let z = oz as f32 + CELL_BLOCKS as f32 * (0.20 + 0.60 * jz);
        let sx = x.round() as i32;
        let sz = z.round() as i32;
        if self.is_ocean_at(sx, sz) {
            return None;
        }
        let elev = self.coarse_elevation(sx, sz);
        if !(SOURCE_MIN_ELEV..=SOURCE_MAX_ELEV).contains(&elev) {
            return None;
        }
        // Score (regional wetness) ranks sources for suppress-weaker-of-two.
        Some(SourceGate { x, z, score: cluster })
    }

    /// Suppress-weaker-of-two (decisions §4): drop this cell's source if a
    /// neighbour within `MIN_SOURCE_SPACING` has a strictly higher score (tiebreak
    /// on `(cx,cz)`). Deterministic + symmetric, so exactly one of a close pair
    /// survives. Pure function of (seed, cell).
    fn source_suppressed(&self, cx: i32, cz: i32, src: SourceGate) -> bool {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (nx, nz) = (cx + dx, cz + dz);
                let Some(other) = self.source_gate(nx, nz) else {
                    continue;
                };
                let ddx = other.x - src.x;
                let ddz = other.z - src.z;
                if ddx * ddx + ddz * ddz >= MIN_SOURCE_SPACING * MIN_SOURCE_SPACING {
                    continue;
                }
                let wins =
                    other.score > src.score || (other.score == src.score && (nz, nx) < (cz, cx));
                if wins {
                    return true;
                }
            }
        }
        false
    }

    /// Memoized wrapper over [`Self::compute_path_from_cell`]. The cache is a pure
    /// memo of a pure function, so results are identical to computing every time.
    fn path_from_cell(&self, cx: i32, cz: i32) -> Option<RiverPath> {
        if let Some(cached) = self.path_cache.read().unwrap().get(&(cx, cz)) {
            return cached.clone();
        }
        let path = self.compute_path_from_cell(cx, cz);
        self.path_cache
            .write()
            .unwrap()
            .insert((cx, cz), path.clone());
        path
    }

    fn compute_path_from_cell(&self, cx: i32, cz: i32) -> Option<RiverPath> {
        let gate = self.source_gate(cx, cz)?;
        if self.source_suppressed(cx, cz, gate) {
            return None;
        }

        let mut rng = FeatureRng::positional(self.seed, SOURCE_SALT, cx, 0, cz);
        // Re-consume the 3 gate draws (roll, jitter x, jitter z) so subsequent
        // draws (phase, dir) stay on the same stream the gate established.
        let _roll = rng.next_f32();
        let _jit_x = rng.next_f32();
        let _jit_z = rng.next_f32();
        let phase0 = rng.next_f32() * std::f32::consts::TAU;

        let mut x = gate.x;
        let mut z = gate.z;
        let mut dir = unit_from_angle(rng.next_f32() * std::f32::consts::TAU);
        let mut points = Vec::with_capacity(PATH_STEPS + 1);
        let mut min_x = x;
        let mut min_z = z;
        let mut max_x = x;
        let mut max_z = z;
        let mut reached_ocean = false;
        let mut ocean_extra = 0usize;

        for step in 0..=PATH_STEPS {
            let downstream = step as f32 / PATH_STEPS as f32;
            let w = self.channel_width(x, z, downstream);
            let depth = self.channel_depth(x, z, w, downstream);
            points.push(RiverPoint {
                x,
                z,
                width: w,
                depth,
            });
            min_x = min_x.min(x);
            min_z = min_z.min(z);
            max_x = max_x.max(x);
            max_z = max_z.max(z);

            // Ocean termination: on first ocean hit, extend a few steps into the
            // sea then stop with the mouth at full width (decisions §3).
            if !reached_ocean && self.is_ocean_at(x.round() as i32, z.round() as i32) {
                reached_ocean = true;
            }
            if reached_ocean {
                ocean_extra += 1;
                if ocean_extra > OCEAN_OVERSHOOT_STEPS {
                    break;
                }
            }

            let s = step as f32 * STEP_BLOCKS;
            dir = self.flow_dir(x, z, dir, s, w, phase0);
            x += dir.0 * STEP_BLOCKS;
            z += dir.1 * STEP_BLOCKS;
        }

        // Cap hit without reaching ocean → terminal pond so it never ends in a
        // dry wide stub (decisions §3).
        if !reached_ocean {
            self.apply_terminal_pond(&mut points);
            for p in points.iter().rev().take(3) {
                min_x = min_x.min(p.x);
                min_z = min_z.min(p.z);
                max_x = max_x.max(p.x);
                max_z = max_z.max(p.z);
            }
        }

        Some(RiverPath {
            key: (cx, cz),
            points,
            min_x: min_x - MAX_QUERY_RADIUS,
            min_z: min_z - MAX_QUERY_RADIUS,
            max_x: max_x + MAX_QUERY_RADIUS,
            max_z: max_z + MAX_QUERY_RADIUS,
        })
    }

    /// Flatten the meander over the last few points and seat a small basin so a
    /// non-ocean-reaching river ends in water.
    fn apply_terminal_pond(&self, points: &mut [RiverPoint]) {
        let n = points.len();
        if n < 2 {
            return;
        }
        let last = points[n - 1];
        let pond_w = (2.0 * WET_MIN).max(0.8 * last.width).min(WET_MAX);
        let pond_d = (BED_MIN_DEPTH + 2.0).min(BED_MAX_DEPTH);
        let span = n.min(3);
        for k in 0..span {
            let idx = n - 1 - k;
            let blend = 1.0 - k as f32 / span as f32; // 1 at terminus, fades inward
            let p = &mut points[idx];
            // Floor at WET_MIN (not a blend-scaled floor) so EVERY pond point is
            // wet — otherwise the upstream-most pond point could drop below WET_MIN
            // and leave a 1-point dry gap between the pond and the river.
            p.width = lerp(p.width, pond_w, blend).max(WET_MIN);
            p.depth = lerp(p.depth, pond_d, blend);
        }
    }

    fn channel_width(&self, x: f32, z: f32, downstream: f32) -> f32 {
        let grow = WET_HEADWATER + smoothstep01(downstream) * (WET_MAX - WET_HEADWATER);
        let fluct = (0.65 * self.width.get([x as f64 * 0.004, z as f64 * 0.004]) as f32
            + 0.35 * self.width.get([x as f64 * 0.013, z as f64 * 0.013]) as f32)
            * 4.0;
        let head = smoothstep(0.0, 0.10, downstream); // source fade only
        ((grow + fluct).max(0.0) * head).clamp(0.0, WET_MAX)
    }

    fn channel_depth(&self, x: f32, z: f32, width: f32, downstream: f32) -> f32 {
        let wob = self.depth.get([x as f64 * 0.007, z as f64 * 0.007]) as f32;
        let base = BED_MIN_DEPTH
            + (width / WET_MAX) * (BED_MAX_DEPTH - BED_MIN_DEPTH)
            + downstream * 1.5
            + wob * 1.0;
        (base * smoothstep(0.0, 0.10, downstream)).clamp(0.0, BED_MAX_DEPTH)
    }

    /// Noisy waterline offset (≥2 octaves), via the shared `bank` sampler at world
    /// coords. Centred near 0 so it widens AND pinches the wet edge.
    fn edge_noise(&self, wx: i32, wz: i32) -> f32 {
        let n0 = self
            .bank
            .get([wx as f64 * 0.020 + 91.0, wz as f64 * 0.020 - 37.0]) as f32;
        let n1 = self
            .bank
            .get([wx as f64 * 0.075 - 17.0, wz as f64 * 0.075 + 53.0]) as f32;
        (0.65 * n0 + 0.35 * n1) * EDGE_AMP
    }

    /// Floodplain undulation (≥2 octaves). Breaks the valley floor so it is never
    /// a flat strip at exactly SEA_LEVEL — some of it dips below (water creeps in),
    /// some rises just above (dry bank). Pure function of world coords.
    fn floodplain_noise(&self, wx: i32, wz: i32) -> f32 {
        let n0 = self
            .bank
            .get([wx as f64 * 0.028 + 401.0, wz as f64 * 0.028 - 263.0]) as f32;
        let n1 = self
            .bank
            .get([wx as f64 * 0.091 - 121.0, wz as f64 * 0.091 + 77.0]) as f32;
        (0.7 * n0 + 0.3 * n1) * FLOODPLAIN_AMP
    }

    fn bank_steepness(&self, wx: i32, wz: i32, biome: Biome, relief: f32) -> f32 {
        let biome_bias = match biome {
            Biome::Mountains
            | Biome::SnowyPeaks
            | Biome::StonyPeaks
            | Biome::WindsweptHills
            | Biome::SnowySlopes => 0.82,
            Biome::Foothills | Biome::Grove | Biome::OldGrowthTaiga => 0.58,
            Biome::Badlands | Biome::Savanna => 0.45,
            Biome::Forest | Biome::BirchForest | Biome::DarkForest | Biome::Jungle => 0.34,
            Biome::Plains | Biome::Meadow | Biome::CherryGrove => 0.24,
            _ => 0.30,
        };
        let relief_bias = smoothstep(10.0, 76.0, relief);
        let noise = self.bank.get([wx as f64 * 0.006, wz as f64 * 0.006]) as f32 * 0.5 + 0.5;
        (biome_bias * 0.56 + relief_bias * 0.30 + noise * 0.14).clamp(0.0, 1.0)
    }

    /// Rim/wall variation whose envelope VANISHES at both joins so the carve meets
    /// untouched terrain seamlessly. `v` is the normalized wall parameter (0 at the
    /// floodplain join, 1 at the rim).
    fn rim_noise(&self, wx: i32, wz: i32, relief: f32, steepness: f32, v: f32) -> f32 {
        let envelope = smoothstep(0.1, 0.4, v) * (1.0 - smoothstep(0.85, 1.0, v));
        if envelope <= 0.0 {
            return 0.0;
        }
        let broad = self
            .bank
            .get([wx as f64 * 0.041 - 177.0, wz as f64 * 0.041 + 53.0]) as f32;
        let detail = self
            .bank
            .get([wx as f64 * 0.137 + 31.0, wz as f64 * 0.137 - 211.0]) as f32;
        let terrace =
            self.bank
                .get([wx as f64 * 0.083 + 307.0, wz as f64 * 0.083 + 149.0]) as f32;
        let amplitude = (1.3 + relief * 0.065 + (1.0 - steepness) * 1.1).clamp(1.0, 6.5);
        let signal = broad * 0.65 + detail * 0.25 + terrace.signum() * 0.18;
        signal * amplitude * envelope
    }

    fn bed_block(&self, wx: i32, wz: i32, biome: Biome) -> Block {
        let material_noise = self.material.get([wx as f64 * 0.0045, wz as f64 * 0.0045]) as f32;
        let sand_bias = match biome {
            Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert => 0.72,
            Biome::Badlands | Biome::Savanna => 0.48,
            Biome::Swamp | Biome::Wetland => 0.12,
            Biome::Mountains | Biome::SnowyPeaks | Biome::StonyPeaks | Biome::WindsweptHills => {
                -0.12
            }
            _ => -0.26,
        };
        let gravel_bias = match biome {
            Biome::Mountains
            | Biome::SnowyPeaks
            | Biome::StonyPeaks
            | Biome::WindsweptHills
            | Biome::Foothills => 0.18,
            _ => -0.20,
        };
        if sand_bias + material_noise * 0.42 > 0.34 {
            Block::Sand
        } else if gravel_bias + material_noise * 0.36 > 0.18 {
            Block::Gravel
        } else if material_noise < -0.34 {
            Block::CoarseDirt
        } else {
            Block::Dirt
        }
    }

    fn bank_block(
        &self,
        wx: i32,
        wz: i32,
        biome: Biome,
        influence: f32,
        width: f32,
    ) -> Option<Block> {
        let deposit_noise =
            self.material
                .get([wx as f64 * 0.0065 + 211.0, wz as f64 * 0.0065 - 109.0]) as f32
                * 0.5
                + 0.5;
        // §8 retune: influence now plateaus near 1 across the channel+floodplain,
        // so the gate knee is pushed out to keep deposits to the inner banks.
        let zone = smoothstep(0.45, 0.9, influence) * smoothstep(4.0, 15.0, width);
        let chance = match biome {
            Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert => 0.88,
            Biome::Badlands => 0.78,
            Biome::Savanna => 0.46,
            Biome::Mountains
            | Biome::SnowyPeaks
            | Biome::StonyPeaks
            | Biome::WindsweptHills
            | Biome::Foothills
            | Biome::SnowySlopes => 0.38,
            Biome::Swamp | Biome::Wetland => 0.14,
            Biome::Plains
            | Biome::Meadow
            | Biome::Forest
            | Biome::BirchForest
            | Biome::DarkForest
            | Biome::Jungle
            | Biome::CherryGrove
            | Biome::Taiga
            | Biome::OldGrowthTaiga => 0.18,
            _ => 0.24,
        } * zone;
        if deposit_noise > chance {
            return None;
        }

        Some(match biome {
            Biome::Badlands => Block::RedSand,
            Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert | Biome::Savanna => {
                Block::Sand
            }
            Biome::Mountains
            | Biome::SnowyPeaks
            | Biome::StonyPeaks
            | Biome::WindsweptHills
            | Biome::Foothills
            | Biome::SnowySlopes => Block::Gravel,
            _ => Block::Gravel,
        })
    }

    /// Terrain-aware flow direction: a seaward target (downhill gradient + global
    /// tilt) perturbed by a bounded arc-length meander, then the turn from the
    /// previous heading is CLAMPED to `MAX_TURN`. Clamping is the key fix over a
    /// raw weighted sum, whose net could reverse/curl and tie the path in knots —
    /// a clamped turn can only ever bend the river gently forward.
    fn flow_dir(
        &self,
        x: f32,
        z: f32,
        prev: (f32, f32),
        s: f32,
        width: f32,
        phase0: f32,
    ) -> (f32, f32) {
        // Wide central difference of the coarse elevation (downhill = -gradient).
        // ~0 inside one biome; nonzero and seaward across a biome boundary.
        let gx = self.coarse_elevation((x + GRAD_OFFS).round() as i32, z.round() as i32)
            - self.coarse_elevation((x - GRAD_OFFS).round() as i32, z.round() as i32);
        let gz = self.coarse_elevation(x.round() as i32, (z + GRAD_OFFS).round() as i32)
            - self.coarse_elevation(x.round() as i32, (z - GRAD_OFFS).round() as i32);
        let downhill = normalize((-gx, -gz)).unwrap_or((0.0, 0.0));

        // Lateral meander: a bounded perpendicular swing about the forward target,
        // arc-length phased so its wavelength scales with width. Because it is a
        // perpendicular COMPONENT (not a free vector) it can never point backward.
        let l = (MEANDER_BASE + MEANDER_K * width).max(1.0);
        let m = (phase0 + std::f32::consts::TAU * s / l).sin();
        let perp = (-prev.1, prev.0);

        // Seaward target heading.
        let desired = normalize((
            downhill.0 * W_DOWN + self.tilt_x * W_TILT + perp.0 * m * W_MEANDER,
            downhill.1 * W_DOWN + self.tilt_z * W_TILT + perp.1 * m * W_MEANDER,
        ))
        .unwrap_or(prev);

        // Clamp the turn from `prev` to `desired` to ±MAX_TURN — guarantees no knots.
        turn_limited(prev, desired, self.cos_max_turn, self.sin_max_turn)
    }
}

/// Rotate unit vector `prev` toward unit vector `desired` by at most `MAX_TURN`,
/// whose cos/sin are passed in. Pure 2-D rotation by a constant angle — no
/// per-step `atan2`, keeping the trig surface (and determinism) minimal.
fn turn_limited(prev: (f32, f32), desired: (f32, f32), cos_max: f32, sin_max: f32) -> (f32, f32) {
    let dot = (prev.0 * desired.0 + prev.1 * desired.1).clamp(-1.0, 1.0);
    if dot >= cos_max {
        return desired; // already within the per-step turn limit
    }
    // Rotate `prev` by ±MAX_TURN, sign chosen to turn toward `desired`.
    let s = if prev.0 * desired.1 - prev.1 * desired.0 >= 0.0 {
        sin_max
    } else {
        -sin_max
    };
    normalize((prev.0 * cos_max - prev.1 * s, prev.0 * s + prev.1 * cos_max)).unwrap_or(prev)
}

#[derive(Clone)]
struct RiverPath {
    /// Stable identity = source cell. Distinct paths have distinct keys; used for
    /// the deterministic two-nearest tiebreak (order-independent).
    key: (i32, i32),
    points: Vec<RiverPoint>,
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
}

impl RiverPath {
    fn intersects(&self, x0: f32, z0: f32, x1: f32, z1: f32) -> bool {
        self.max_x >= x0 && self.min_x <= x1 && self.max_z >= z0 && self.min_z <= z1
    }
}

#[derive(Copy, Clone)]
struct RiverPoint {
    x: f32,
    z: f32,
    width: f32,
    depth: f32,
}

#[derive(Copy, Clone)]
struct RiverHit {
    distance: f32,
    width: f32,
    depth: f32,
    key: (i32, i32),
}

/// The two nearest hits from DISTINCT paths (best, second-best by path identity),
/// each the closest segment of its path. Ordering is by `(distance, key)`
/// lexicographically so the result is independent of path iteration order — the
/// seam-determinism guarantee for the two-nearest median carve.
fn nearest_hits(x: f32, z: f32, paths: &[RiverPath]) -> (Option<RiverHit>, Option<RiverHit>) {
    let mut best: Option<RiverHit> = None;
    let mut second: Option<RiverHit> = None;
    for path in paths {
        if x < path.min_x || x > path.max_x || z < path.min_z || z > path.max_z {
            continue;
        }
        // Closest segment of THIS path.
        let mut path_hit: Option<RiverHit> = None;
        for segment in path.points.windows(2) {
            let a = segment[0];
            let b = segment[1];
            let abx = b.x - a.x;
            let abz = b.z - a.z;
            let len2 = abx * abx + abz * abz;
            if len2 <= f32::EPSILON {
                continue;
            }
            let t = (((x - a.x) * abx + (z - a.z) * abz) / len2).clamp(0.0, 1.0);
            let px = a.x + abx * t;
            let pz = a.z + abz * t;
            let dx = x - px;
            let dz = z - pz;
            let distance = (dx * dx + dz * dz).sqrt();
            if distance > MAX_QUERY_RADIUS {
                continue;
            }
            let hit = RiverHit {
                distance,
                width: a.width + (b.width - a.width) * t,
                depth: a.depth + (b.depth - a.depth) * t,
                key: path.key,
            };
            if path_hit.is_none_or(|h| hit_lt(&hit, &h)) {
                path_hit = Some(hit);
            }
        }
        let Some(hit) = path_hit else { continue };
        // Insert into (best, second) by distinct-path ordering.
        if best.is_none_or(|b| hit_lt(&hit, &b)) {
            second = best;
            best = Some(hit);
        } else if second.is_none_or(|s| hit_lt(&hit, &s)) {
            second = Some(hit);
        }
    }
    (best, second)
}

/// Strict lexicographic order on `(distance, key)` — total + deterministic.
#[inline]
fn hit_lt(a: &RiverHit, b: &RiverHit) -> bool {
    if a.distance != b.distance {
        a.distance < b.distance
    } else {
        a.key < b.key
    }
}

#[inline]
fn unit_from_angle(angle: f32) -> (f32, f32) {
    (angle.cos(), angle.sin())
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
    fn straight_path(width: f32, depth: f32) -> RiverPath {
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
                if region.surf[i] <= SEA_LEVEL - 1 {
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
    fn carve_only_never_raises() {
        let rivers = RiverSystem::new(7);
        let paths = [straight_path(16.0, 5.0)];
        for base in [SEA_LEVEL - 3, SEA_LEVEL, SEA_LEVEL + 8, SEA_LEVEL + 40] {
            for wx in (-200..=200).step_by(7) {
                for wz in (-60..=60).step_by(11) {
                    if let Some((_, carved_surf)) =
                        rivers.carve_column(wx, wz, base, Biome::Plains, &paths)
                    {
                        assert!(
                            carved_surf <= base,
                            "carve raised terrain at ({wx},{wz}) base {base} -> {carved_surf}"
                        );
                    }
                }
            }
        }
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
    fn valley_not_ditch() {
        // A high-relief column: the cross-section must be a valley — rim above the
        // floodplain above the waterline — and rise monotonically outward. The
        // path runs along x at z=0, so perpendicular distance == |wz|; sweep wz.
        let rivers = RiverSystem::new(12_345);
        let base = SEA_LEVEL + 20; // relief 20 >= 12
        let paths = [straight_path(14.0, 5.0)];

        // wx = 0 so the sweep passes through the true centerline at wz=0 (distance
        // 0), giving a genuine channel-bed/waterline sample as the nearest point.
        let wx = 0;
        let mut samples = Vec::new(); // (distance, carved_surf)
        for wz in 0..=120 {
            if let Some((river, carved)) = rivers.carve_column(wx, wz, base, Biome::Plains, &paths)
            {
                samples.push((river.distance, carved));
            }
        }
        assert!(samples.len() > 20, "should carve a wide cross-section");
        samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let waterline = samples.first().unwrap().1; // channel bed at the centerline
        let rim = samples.last().unwrap().1; // furthest carved (valley wall top)
                                             // A representative floodplain sample: roughly mid-profile.
        let mid = samples[samples.len() / 2].1;
        assert!(
            waterline <= SEA_LEVEL - 1,
            "channel centre ({waterline}) must carve to/below the waterline"
        );
        assert!(
            rim - mid >= 3,
            "rim ({rim}) should sit well above the floodplain ({mid})"
        );
        assert!(
            mid - waterline >= 1,
            "floodplain ({mid}) should sit above the waterline ({waterline})"
        );
        // Rises outward (valley, not a re-rising ditch lip). Allow small noise
        // jitter (<= 2 blocks) without counting it a violation.
        let mut peak = samples[0].1;
        let mut violations = 0;
        for (_, y) in &samples {
            if *y < peak - 2 {
                violations += 1;
            }
            peak = (*y).max(peak);
        }
        assert_eq!(
            violations, 0,
            "valley profile should rise monotonically outward"
        );
    }

    #[test]
    fn wet_width_within_band_and_fluctuates() {
        // Build real paths and confirm wet widths land in band and vary per path.
        let rivers = RiverSystem::new(12_345);
        let mut found = 0usize;
        'cells: for cz in -6..=6 {
            for cx in -6..=6 {
                let Some(path) = rivers.path_from_cell(cx, cz) else {
                    continue;
                };
                let widths: Vec<f32> = path
                    .points
                    .iter()
                    .map(|p| p.width)
                    .filter(|&w| w >= WET_MIN)
                    .collect();
                if widths.len() < 6 {
                    continue;
                }
                let max = widths.iter().cloned().fold(f32::MIN, f32::max);
                let min = widths.iter().cloned().fold(f32::MAX, f32::min);
                assert!(max <= WET_MAX + 0.001, "wet width {max} exceeds band");
                assert!(min >= 4.0, "wet width {min} below band floor");
                assert!(
                    max - min >= 3.0,
                    "wet width should fluctuate along a path (spread {:.2})",
                    max - min
                );
                found += 1;
                if found >= 3 {
                    break 'cells;
                }
            }
        }
        assert!(found >= 3, "should find several wet paths to measure");
    }

    #[test]
    fn headwater_fades_but_mouth_stays_wide() {
        let rivers = RiverSystem::new(12_345);
        let mut path = None;
        'search: for cz in -10..=10 {
            for cx in -10..=10 {
                if let Some(found) = rivers.path_from_cell(cx, cz) {
                    // Want a sufficiently long, wide river to assert on.
                    if found.points.iter().any(|p| p.width > 12.0) {
                        path = Some(found);
                        break 'search;
                    }
                }
            }
        }
        let path = path.expect("search area should contain a wide generated river path");

        let first = path.points.first().unwrap();
        let last = path.points.last().unwrap();
        // Widest point in the downstream half (robust to per-point pinch noise).
        let half = path.points.len() / 2;
        let mid_max = path.points[half..]
            .iter()
            .map(|p| p.width)
            .fold(0.0f32, f32::max);
        assert!(
            first.width < WET_MIN,
            "source end should start as a sub-WET_MIN trickle (was {})",
            first.width
        );
        assert!(
            mid_max > 12.0,
            "downstream half of a generated river should be visibly wide (was {mid_max})"
        );
        assert!(
            last.width >= WET_MIN,
            "mouth/terminus must stay wide, not taper to nothing (was {})",
            last.width
        );
    }

    #[test]
    fn two_parallel_paths_flood_the_median() {
        // Two near-parallel channels whose wet zones nearly meet: the median
        // between them must flood (the two-nearest carve takes the deepest target)
        // — no dry above-sea sandbar strip.
        let rivers = RiverSystem::new(1);
        let make = |z: f32, key: (i32, i32)| RiverPath {
            key,
            points: vec![
                RiverPoint {
                    x: -300.0,
                    z,
                    width: 12.0,
                    depth: 5.0,
                },
                RiverPoint {
                    x: 300.0,
                    z,
                    width: 12.0,
                    depth: 5.0,
                },
            ],
            min_x: -300.0 - MAX_QUERY_RADIUS,
            min_z: z - MAX_QUERY_RADIUS,
            max_x: 300.0 + MAX_QUERY_RADIUS,
            max_z: z + MAX_QUERY_RADIUS,
        };
        // Centerlines 10 apart; wet_half = 6 each, so the wet zones overlap at the
        // median (z=0 is distance 5 < edge from both).
        let paths = [make(-5.0, (0, 0)), make(5.0, (1, 0))];

        let (river, carved) = rivers
            .carve_column(0, 0, SEA_LEVEL + 6, Biome::Plains, &paths)
            .expect("median column should be carved by two flanking paths");
        assert!(
            carved <= SEA_LEVEL - 1,
            "median between two channels should carve to/below water (was {carved})"
        );
        assert!(river.wet(), "median should be a wet river column");

        // Sweep the whole median strip: none should be a dry above-sea sandbar.
        let mut dry_median = 0usize;
        for wx in (-260..=260).step_by(5) {
            if let Some((_, c)) = rivers.carve_column(wx, 0, SEA_LEVEL + 6, Biome::Plains, &paths) {
                if c > SEA_LEVEL {
                    dry_median += 1;
                }
            }
        }
        assert_eq!(dry_median, 0, "no dry mid-channel sandbar should remain");
    }

    #[test]
    fn turn_limited_clamps_and_stays_unit() {
        // The headline no-knot guarantee: the heading can never change by more than
        // MAX_TURN in a step, and stays a unit vector.
        let (c, s) = (MAX_TURN.cos(), MAX_TURN.sin());
        let prev = (1.0f32, 0.0);
        for &desired in &[(-1.0f32, 0.0f32), (0.0, 1.0), (0.0, -1.0)] {
            let out = turn_limited(prev, desired, c, s);
            let len = (out.0 * out.0 + out.1 * out.1).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "heading must stay unit (len {len})");
            let ang = (prev.0 * out.0 + prev.1 * out.1).clamp(-1.0, 1.0).acos();
            assert!(
                ang <= MAX_TURN + 1e-3,
                "turn {ang} exceeded MAX_TURN {MAX_TURN}"
            );
        }
        // A small desired turn (within the limit) is applied directly.
        let small = normalize((1.0, 0.1)).unwrap();
        let out = turn_limited(prev, small, c, s);
        assert!((out.0 - small.0).abs() < 1e-6 && (out.1 - small.1).abs() < 1e-6);
    }

    #[test]
    fn generated_paths_meander() {
        // At least one generated river should visibly wander (sinuosity = arc length
        // / straight-line distance well above 1), proving the meander actually bends
        // the course rather than running straight.
        let rivers = RiverSystem::new(12_345);
        let mut best_sinuosity = 1.0f32;
        for cz in -8..=8 {
            for cx in -8..=8 {
                let Some(path) = rivers.path_from_cell(cx, cz) else {
                    continue;
                };
                if path.points.len() < 10 {
                    continue;
                }
                let arc: f32 = path
                    .points
                    .windows(2)
                    .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].z - w[0].z).powi(2)).sqrt())
                    .sum();
                let f = path.points.first().unwrap();
                let l = path.points.last().unwrap();
                let straight = ((l.x - f.x).powi(2) + (l.z - f.z).powi(2)).sqrt();
                if straight < 80.0 {
                    continue; // skip short/pond-terminated degenerate paths
                }
                best_sinuosity = best_sinuosity.max(arc / straight);
            }
        }
        assert!(
            best_sinuosity >= 1.1,
            "at least one river should visibly meander (best sinuosity {best_sinuosity})"
        );
    }

    #[test]
    fn bank_carve_fluctuates_around_smoothed_terrain() {
        let rivers = RiverSystem::new(12_345);
        let paths = [straight_path(16.0, 6.0)];

        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut samples = 0usize;
        for wx in (-320..=320).step_by(8) {
            let Some((river, carved_surf)) =
                rivers.carve_column(wx, 30, SEA_LEVEL + 32, Biome::Plains, &paths)
            else {
                continue;
            };
            assert!(river.active());
            assert!(
                (3..=SEA_LEVEL + 32).contains(&carved_surf),
                "bank variation should stay between the bed floor and the pre-river terrain"
            );
            min_y = min_y.min(carved_surf);
            max_y = max_y.max(carved_surf);
            samples += 1;
        }

        assert!(samples > 32, "synthetic bank should produce enough samples");
        assert!(
            max_y - min_y >= 3,
            "constant-height input terrain should still produce varied river surface heights"
        );
    }

    #[test]
    fn steeper_biomes_give_steeper_walls() {
        // bank_extra is gone; steepness now drives the wall exponent. A steeper
        // biome must produce a taller wall partway up (concave-up `v^wall_exp`).
        let rivers = RiverSystem::new(12_345);
        let plains = rivers.bank_steepness(0, 0, Biome::Plains, 10.0);
        let mountains = rivers.bank_steepness(0, 0, Biome::Mountains, 80.0);
        assert!(
            mountains > plains,
            "mountainous terrain should bias toward steeper river banks"
        );

        // wall_exp = 1 + steepness*0.8; for v in (0,1), higher exp => smaller value
        // at the same v => the wall stays lower until close to the rim (steeper at
        // the top). Verify the exponent relationship directly via the carve.
        let v = 0.5f32;
        let gentle = v.powf(1.0 + plains * 0.8);
        let steep = v.powf(1.0 + mountains * 0.8);
        assert!(
            steep < gentle,
            "steeper banks should hug the floodplain longer then rise sharply"
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
                if rivers.bed_block(wx, wz, Biome::Desert) == Block::Sand {
                    desert_sand += 1;
                }
                match rivers.bed_block(wx, wz, Biome::Plains) {
                    Block::Sand => plains_sand += 1,
                    Block::Dirt | Block::CoarseDirt => plains_soil += 1,
                    _ => {}
                }
                if rivers.bed_block(wx, wz, Biome::Mountains) == Block::Gravel {
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

    #[test]
    fn river_through_existing_water_preserves_bed_material_flag() {
        let rivers = RiverSystem::new(7);
        let path = RiverPath {
            key: (0, 0),
            points: vec![
                RiverPoint {
                    x: -32.0,
                    z: 0.0,
                    width: 16.0,
                    depth: 5.0,
                },
                RiverPoint {
                    x: 32.0,
                    z: 0.0,
                    width: 18.0,
                    depth: 5.5,
                },
            ],
            min_x: -MAX_QUERY_RADIUS,
            min_z: -MAX_QUERY_RADIUS,
            max_x: MAX_QUERY_RADIUS,
            max_z: MAX_QUERY_RADIUS,
        };

        let (river, carved_surf) = rivers
            .carve_column(0, 0, SEA_LEVEL - 1, Biome::Plains, &[path])
            .expect("centerline should carve the shallow water-body floor");

        assert!(river.wet());
        assert!(river.preserve_bed);
        assert_eq!(river.water_y, SEA_LEVEL);
        assert!(
            carved_surf < SEA_LEVEL - 1,
            "river should clear shallow water-body floors to its channel bed"
        );
    }
}
