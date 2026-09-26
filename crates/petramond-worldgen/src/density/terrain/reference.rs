//! The imperative terrain recipe the shipped `assets/density/terrain.json` was
//! generated from — kept as the TEST REFERENCE (the live recipe is data, see
//! [`crate::data::terrain`]). The data recipe must evaluate bit-identically to
//! this builder on every channel.

use super::super::shaper;
use super::channels;
use super::TerrainDensityGraph;
use crate::density::noise::{climate_fields, ShiftedClimateField};
use crate::graph::spline::{CubicSpline, SplineAxis, SplinePoint};
use crate::graph::{Channel, NodeId, ScalarGraph};

/// Blocks of surface height per unit of continent offset (the inverse of the
/// vertical depth-gradient slope).
pub(super) const HEIGHT_SCALE: f64 = 128.0;
/// Peak extra height (blocks) the crag term can add on fully-weighted crests.
/// Tuned against `genmap rough`: 34 pushed a mountain-heavy seed to 82%
/// walkable (the floor is 80%); 28 keeps crests dramatic with margin.
const CRAG_AMPLITUDE: f64 = 28.0;
/// Mesa terrace tread height in blocks.
const MESA_TERRACE_STEP: f64 = 8.0;
/// Cap on how fully the terraced profile replaces the smooth one in mesa
/// country (1.0 = pure staircase; keep some slope so it reads natural).
const MESA_TERRACE_STRENGTH: f64 = 0.85;

/// How sharply the structure field's zero lines become spines: the belt is
/// `|n| < 1/sharpness` wide before squaring.
const STRUCTURE_SPINE_SHARPNESS: f64 = 5.0;
/// Continentality raise on a fully-weighted range spine (pushes coastal spine
/// land inland-ward so belts read as ranges, not island chains).
const STRUCTURE_CONTINENTALITY_UPLIFT: f64 = 0.22;
/// Erosion cut on a fully-weighted spine (low erosion = mountain shaping AND
/// mountain biome classification — the coherence lever).
const STRUCTURE_EROSION_CUT: f64 = 0.50;

/// Style-weight spline axes for channels the reference shaper has no axis
/// names for.
const STYLE_AXIS_OFFSET: &str = "style_offset";
const STYLE_AXIS_VARIANCE: &str = "style_variance";
const STYLE_AXIS_TEMPERATURE: &str = "style_temperature";
const STYLE_AXIS_HUMIDITY: &str = "style_humidity";
const STYLE_AXIS_RAW_CONT: &str = "style_raw_continentality";

/// A 0→1 smoothstep over `[lo, hi]` as a two-point Hermite spline (derivative 0
/// at both ends is exactly smoothstep between the knots, clamped outside).
fn smoothstep_spline(axis: &str, lo: f64, hi: f64) -> CubicSpline {
    CubicSpline::new(
        axis,
        vec![
            SplinePoint::constant_with_derivative(lo, 0.0, 0.0),
            SplinePoint::constant_with_derivative(hi, 1.0, 0.0),
        ],
    )
}

/// A 1→0 smoothstep over `[lo, hi]`.
fn smoothstep_spline_desc(axis: &str, lo: f64, hi: f64) -> CubicSpline {
    CubicSpline::new(
        axis,
        vec![
            SplinePoint::constant_with_derivative(lo, 1.0, 0.0),
            SplinePoint::constant_with_derivative(hi, 0.0, 0.0),
        ],
    )
}

/// 0 inside the centre `|value| < lo` band, 1 beyond `|value| > hi` — keeps a
/// stylization term out of the river/valley variance band.
fn valley_guard_spline(axis: &str, lo: f64, hi: f64) -> CubicSpline {
    CubicSpline::new(
        axis,
        vec![
            SplinePoint::constant_with_derivative(-hi, 1.0, 0.0),
            SplinePoint::constant_with_derivative(-lo, 0.0, 0.0),
            SplinePoint::constant_with_derivative(lo, 0.0, 0.0),
            SplinePoint::constant_with_derivative(hi, 1.0, 0.0),
        ],
    )
}
/// The reference depth datum: `1 − 83/160 + 0.015 = 0.49625`, folded so the
/// offset-0 surface lands at `HEIGHT_SCALE·(1 − DEPTH_OFFSET_BIAS) ≈ 63.5`, just
/// above the reference waterline (sea level 63).
pub(super) const DEPTH_OFFSET_BIAS: f64 = 0.5037500262260437;

#[derive(Clone, Debug)]
pub struct ReferenceTerrainSpec {
    pub shaping: ShapingSplineSpecs,
    pub floor: FloorDensitySpec,
}

impl ReferenceTerrainSpec {
    pub fn default_surface() -> Self {
        Self {
            shaping: ShapingSplineSpecs::default_surface(),
            floor: FloorDensitySpec::default_surface(),
        }
    }

    pub fn build_graph(&self, seed: u32) -> TerrainDensityGraph {
        let mut graph = ScalarGraph::new();

        // Climate axes use the reference's exact double-Perlin noise + domain-warp
        // shift, forked from the world seed. The game's u32 seed widens to the
        // reference's u64 world seed, so the same seed yields the same fields.
        let world_seed = u64::from(seed);
        let temperature = graph.sampled_field(ShiftedClimateField::new(
            world_seed,
            &climate_fields::TEMPERATURE,
        ));
        let humidity = graph.sampled_field(ShiftedClimateField::new(
            world_seed,
            &climate_fields::HUMIDITY,
        ));
        let continentality = graph.sampled_field(ShiftedClimateField::new(
            world_seed,
            &climate_fields::CONTINENTALITY,
        ));
        let erosion = graph.sampled_field(ShiftedClimateField::new(
            world_seed,
            &climate_fields::EROSION,
        ));
        let variance = graph.sampled_field(ShiftedClimateField::new(
            world_seed,
            &climate_fields::WEIRDNESS,
        ));

        // --- World structure (ours, not a reference port) -----------------------
        // Kilometres-scale mountain-range spines: the STRUCTURE field's zero
        // lines, ridge-folded into an uplift belt, RAISE continentality and
        // LOWER erosion along connected kilometre-long paths. Because the
        // modulated channels feed BOTH the offset spline and the biome
        // classifier (ClimateSampler reads these graph channels), ranges rise
        // as belts AND get classified as mountain biomes — coherence is
        // structural. The uplift fades out toward deep ocean so open sea stays
        // sea (coastal spines still make dramatic peninsulas).
        let structure_raw = graph.sampled_field(ShiftedClimateField::new(
            world_seed,
            &climate_fields::STRUCTURE,
        ));
        let structure_abs = graph.abs(structure_raw);
        let spine_slope = graph.constant(-STRUCTURE_SPINE_SHARPNESS);
        let spine_one = graph.constant(1.0);
        let spine_folded = graph.multiply(structure_abs, spine_slope);
        let spine_ridge = graph.add(spine_folded, spine_one);
        let spine = graph.clamp(spine_ridge, 0.0, 1.0);
        let spine_sq = graph.multiply(spine, spine);
        let w_landward = graph.spline(
            smoothstep_spline(STYLE_AXIS_RAW_CONT, -0.45, -0.15),
            vec![(SplineAxis::new(STYLE_AXIS_RAW_CONT), continentality)],
        );
        let uplift = graph.multiply(spine_sq, w_landward);

        let uplift_cont_amp = graph.constant(STRUCTURE_CONTINENTALITY_UPLIFT);
        let uplift_cont = graph.multiply(uplift, uplift_cont_amp);
        let continentality = graph.add(continentality, uplift_cont);

        let uplift_ero_amp = graph.constant(-STRUCTURE_EROSION_CUT);
        let uplift_ero = graph.multiply(uplift, uplift_ero_amp);
        let erosion = graph.add(erosion, uplift_ero);

        graph.set_channel(Channel::new(channels::TEMPERATURE), temperature);
        graph.set_channel(Channel::new(channels::HUMIDITY), humidity);
        graph.set_channel(Channel::new(channels::CONTINENTALITY), continentality);
        graph.set_channel(Channel::new(channels::EROSION), erosion);
        graph.set_channel(Channel::new(channels::VARIANCE), variance);

        let ridge = graph.ridge_fold(variance);
        graph.set_channel(Channel::new(channels::RIDGE), ridge);

        let offset = graph.spline(
            self.shaping.offset.clone(),
            shaping_inputs(continentality, erosion, ridge),
        );

        // --- Stylized silhouette (ours, not a reference port) -------------------
        // Two shaping terms ride on top of the reference offset, both weighted by
        // functions of the SAME climate channels that classify biomes, so terrain
        // character and biome identity stay coupled:
        //
        // 1. CRAG: connected ridge crests on high, low-erosion terrain. A ridged
        //    transform (1 − 2|n|, creases along the field's zero lines) of our
        //    dedicated crag field, faded in by offset height and out by erosion,
        //    and gated to zero in the centre variance band so river valleys stay
        //    valleys.
        let crag_raw =
            graph.sampled_field(ShiftedClimateField::new(world_seed, &climate_fields::CRAG));
        let crag_abs = graph.abs(crag_raw);
        let neg_two = graph.constant(-2.0);
        let one = graph.constant(1.0);
        let crag_folded = graph.multiply(crag_abs, neg_two);
        let crag_ridge = graph.add(crag_folded, one);
        let crest = graph.clamp(crag_ridge, 0.0, 1.0);
        let crest_sq = graph.multiply(crest, crest);
        let w_high = graph.spline(
            smoothstep_spline(STYLE_AXIS_OFFSET, 0.30, 0.55),
            vec![(SplineAxis::new(STYLE_AXIS_OFFSET), offset)],
        );
        let w_uneroded = graph.spline(
            smoothstep_spline_desc(shaper::axes::EROSION, -0.60, 0.05),
            vec![(SplineAxis::new(shaper::axes::EROSION), erosion)],
        );
        let w_off_river = graph.spline(
            valley_guard_spline(STYLE_AXIS_VARIANCE, 0.06, 0.16),
            vec![(SplineAxis::new(STYLE_AXIS_VARIANCE), variance)],
        );
        let crag_weight_he = graph.multiply(w_high, w_uneroded);
        let crag_weight = graph.multiply(crag_weight_he, w_off_river);
        let crag_amp = graph.constant(CRAG_AMPLITUDE / HEIGHT_SCALE);
        let crag_gain = graph.multiply(crest_sq, crag_amp);
        let crag_term = graph.multiply(crag_gain, crag_weight);
        let styled_offset = graph.add(offset, crag_term);

        // Reference surface height is the depth crossing `d = 0`, which solves to
        // `y = 128·(1 − 0.50375 + offset)`. The constant folds the reference depth
        // datum (`1 − 83/160 + 0.015 = 0.49625`); `offset` is the reference spline
        // plus the stylized crag term. Sea level (63) sits just below the offset-0
        // land at ≈63.5.
        let height_scale = graph.constant(HEIGHT_SCALE);
        let sea_offset = graph.constant(HEIGHT_SCALE * (1.0 - DEPTH_OFFSET_BIAS));
        let scaled_offset = graph.multiply(styled_offset, height_scale);
        let smooth_height = graph.add(scaled_offset, sea_offset);

        // 2. MESA terraces: hot + dry + eroded land steps into flat treads with
        //    steep risers. The terrace is blended in by a hot×dry×eroded×inland
        //    weight, so deserts/savanna badlands terrace while everything else
        //    keeps the smooth profile.
        let terraced = graph.terrace(smooth_height, MESA_TERRACE_STEP);
        let w_hot = graph.spline(
            smoothstep_spline(shaper::axes::CONTINENTALITY, -0.05, 0.15),
            vec![(
                SplineAxis::new(shaper::axes::CONTINENTALITY),
                continentality,
            )],
        );
        let w_temp = graph.spline(
            smoothstep_spline(STYLE_AXIS_TEMPERATURE, 0.40, 0.60),
            vec![(SplineAxis::new(STYLE_AXIS_TEMPERATURE), temperature)],
        );
        let w_dry = graph.spline(
            smoothstep_spline_desc(STYLE_AXIS_HUMIDITY, -0.35, -0.05),
            vec![(SplineAxis::new(STYLE_AXIS_HUMIDITY), humidity)],
        );
        let w_eroded = graph.spline(
            smoothstep_spline(shaper::axes::EROSION, -0.10, 0.25),
            vec![(SplineAxis::new(shaper::axes::EROSION), erosion)],
        );
        let mesa_td = graph.multiply(w_temp, w_dry);
        let mesa_tde = graph.multiply(mesa_td, w_eroded);
        let mesa_raw = graph.multiply(mesa_tde, w_hot);
        let mesa_strength = graph.constant(MESA_TERRACE_STRENGTH);
        let mesa_weight = graph.multiply(mesa_raw, mesa_strength);
        let base_height = graph.lerp(smooth_height, terraced, mesa_weight);
        graph.set_channel(Channel::new(channels::BASE_HEIGHT), base_height);

        // The reference height model is depth-only: the surface is exactly the
        // depth crossing, with no 3D detail, squash factor, or jaggedness (those
        // shape the full density function, which the reference surface height does
        // not model). master_density therefore crosses zero precisely at
        // base_height, so the top-solid lattice scan recovers the reference height.
        let vertical_bias = graph.vertical_bias(base_height);
        let master_density = graph.floor_clamp(
            vertical_bias,
            self.floor.floor_y,
            self.floor.fade_height,
            self.floor.solid_density,
        );
        graph.set_channel(Channel::new(channels::MASTER_DENSITY), master_density);

        let surface_detection = graph.constant(0.0);
        graph.set_channel(Channel::new(channels::SURFACE_DETECTION), surface_detection);

        TerrainDensityGraph { graph }
    }
}

#[derive(Clone, Debug)]
pub struct ShapingSplineSpecs {
    /// Continent height offset (scaled into `base_height`). This is the only
    /// shaping input the reference surface height needs: the surface is the
    /// depth-zero crossing, with no squash factor or jaggedness (those shape the
    /// full density function, not the surface height).
    pub offset: CubicSpline,
}

impl ShapingSplineSpecs {
    pub fn default_surface() -> Self {
        Self {
            offset: shaper::offset_spline(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FloorDensitySpec {
    pub floor_y: f64,
    pub fade_height: f64,
    pub solid_density: f64,
}

impl FloorDensitySpec {
    pub const fn new(floor_y: f64, fade_height: f64, solid_density: f64) -> Self {
        Self {
            floor_y,
            fade_height,
            solid_density,
        }
    }

    pub const fn default_surface() -> Self {
        Self::new(0.0, 8.0, 64.0)
    }
}

fn shaping_inputs(
    continentality: NodeId,
    erosion: NodeId,
    ridge: NodeId,
) -> Vec<(SplineAxis, NodeId)> {
    vec![
        (
            SplineAxis::new(shaper::axes::CONTINENTALITY),
            continentality,
        ),
        (SplineAxis::new(shaper::axes::EROSION), erosion),
        (SplineAxis::new(shaper::axes::RIDGE), ridge),
    ]
}

