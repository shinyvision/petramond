//! Const noise-sampler settings — the float-parity oracle for `HeightField`.
//!
//! Salts, octaves, and frequencies copied verbatim from the pre-Strata
//! `gen.rs::WorldNoise::new`. These are the single source of truth for sampler
//! construction; changing any value here changes generated terrain (and breaks
//! the genparity gate by design).

/// Per-sampler seed salts, added to the world seed. Verbatim from gen.rs.
pub const SALT_TEMP: u32 = 0x111;
pub const SALT_HUMID: u32 = 0x222;
pub const SALT_CONT: u32 = 0x333;
pub const SALT_EROS: u32 = 0x444;
pub const SALT_WEIRD: u32 = 0x555;
pub const SALT_DEPTH: u32 = 0x666;
pub const SALT_JAG: u32 = 0x777;
pub const SALT_SURF: u32 = 0x888;
pub const SALT_OFF: u32 = 0x999;
pub const SALT_RIVER: u32 = 0xAAA;
pub const SALT_RIVERW: u32 = 0xBBB;

// Fractal sampler shapes (octaves, frequency), verbatim from gen.rs. NOTE these
// set each fractal's *internal* base frequency; `surface_height`/`river_strength`
// additionally scale their sample coordinates (e.g. `[fx * 0.012, ...]`), so the
// per-call coordinate multipliers in `height.rs` are a separate, equally
// load-bearing set of constants and must not be folded into these.
pub const CONT_OCTAVES: usize = 3;
pub const CONT_FREQ: f64 = 0.0013;
pub const WEIRD_OCTAVES: usize = 4;
pub const WEIRD_FREQ: f64 = 0.0055;
pub const JAG_OCTAVES: usize = 3;
pub const JAG_FREQ: f64 = 0.012;

// River network. A smooth fbm whose ZERO-CONTOUR is the river: `river_strength`
// carves where |sample| is small, so rivers are the long winding curves of the
// noise's zero set (connected meanders), not a threshold on a ridged sampler
// (whose output never reaches 0 — the old bug that left rivers uncarved).
// Frequency is LITERAL: `river_strength` samples with raw world coords (no extra
// coordinate multiplier), so this value IS the period (~1/f blocks) — unlike the
// legacy samplers that double-scale via both set_frequency AND a `.get([x*m])`
// multiplier. `RIVER_HALF` is the valley half-width in BLOCKS: `river_strength`
// is 1 at the channel centre (the noise zero-contour) and ramps to 0 at this many
// blocks away, measured by dividing |n| by the local gradient so width is
// spatially consistent (a fixed |n| band would balloon wherever the field is
// locally flat). The carver slopes the floor across that band so only the deep
// centre floods — a narrow channel inside a wide, constant-slope valley.
pub const RIVER_OCTAVES: usize = 2;
pub const RIVER_FREQ: f64 = 0.000_75;
pub const RIVER_HALF: f32 = 15.0;

// River meander + width variation. The clean zero-contour of a smooth fbm draws
// geometric arcs; domain-warping the sample point (a medium-frequency field
// displacing coords by ~RIVER_WARP_AMP blocks) bends it into natural meanders
// (and the occasional oxbow). `RIVER_WIDTH_VAR` modulates the half-width along
// the course (wider pools, narrower runs) so banks aren't parallel lines.
pub const RIVER_WARP_FREQ: f64 = 0.018;
pub const RIVER_WARP_AMP: f64 = 26.0;
pub const RIVER_WIDTH_VAR: f32 = 0.35;

// --- Worldgen v2 ("Strata-Relief II") additions ---

/// 3-D overhang carve: a single bare OpenSimplex sampled once per band voxel.
/// Horizontal frequency is broad (wide shelves); vertical is finer so the warped
/// surface leans and undercuts into stacked shelves / overhangs.
pub const SALT_DENS3D: u32 = 0xB01;
pub const DENS3D_FREQ_XZ: f64 = 0.018;
pub const DENS3D_FREQ_Y: f64 = 0.030;

/// Peaks & valleys: broad 2-D mountain massing fbm, gated to highland terrain.
pub const SALT_PV: u32 = 0xC01;
pub const PV_OCTAVES: usize = 3;
pub const PV_FREQ: f64 = 0.0017;

/// Crag ridgelines: a ridged multifractal that adds craggy relief on mountains.
/// Kept BROAD on purpose — only 2 octaves at a ~77-block base period — so the
/// ridges read as walkable craggy slopes, NOT a field of 1-wide pillars (which
/// is what high-frequency / many-octave ridged noise produces in a heightfield,
/// because adjacent columns then differ by a whole spike). Gated to rugged peaks
/// in `surface_height`. Sampled with a unit multiplier (`get([wx, wz])`) so the
/// `set_frequency` value IS the real frequency — unlike the legacy samplers.
pub const SALT_CRAG: u32 = 0xD01;
pub const CRAG_OCTAVES: usize = 2;
pub const CRAG_FREQ: f64 = 0.013;

/// Domain-warp source frequency: a low-frequency field (~366-block period) that
/// displaces the mountain-mass and crag sample points so massifs bend into
/// irregular ridge systems instead of radially-symmetric smooth domes.
pub const WARP_FREQ: f64 = 0.0042;
pub const WARP_AMP: f64 = 64.0;
