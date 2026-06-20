//! Const noise-sampler settings for `HeightField`.
//!
//! Salts, octaves, and frequencies are the single source of truth for sampler
//! construction. Changing any value here changes generated terrain by design.

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

// Fractal sampler shapes (octaves, frequency), verbatim from gen.rs. NOTE these
// set each fractal's *internal* base frequency; `surface_height` additionally
// scales some sample coordinates (e.g. `[fx * 0.012, ...]`), so the per-call
// coordinate multipliers in `height.rs` are a separate, equally load-bearing set
// of constants and must not be folded into these.
pub const CONT_OCTAVES: usize = 3;
pub const CONT_FREQ: f64 = 0.0013;
pub const WEIRD_OCTAVES: usize = 4;
pub const WEIRD_FREQ: f64 = 0.0055;
pub const JAG_OCTAVES: usize = 3;
pub const JAG_FREQ: f64 = 0.012;

// Climate and landform sample frequencies. Temperature/humidity stay broad
// enough for readable climate provinces, but not so broad that one forest or
// savanna dominates an entire generated map. Erosion/depth are shared by biome
// selection and height shaping; keep these aligned with `HeightField::climate`.
pub const TEMP_SAMPLE_FREQ: f64 = 0.00160;
pub const HUMID_SAMPLE_FREQ: f64 = 0.00195;
pub const EROSION_SAMPLE_FREQ: f64 = 0.000_80;
pub const DEPTH_SAMPLE_FREQ: f64 = 0.0011;

// --- Relief shaping additions ---

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

// --- Caves ---
//
// Two flavours, both pure 3-D noise functions of world position (so caves are
// seamless across chunks with no inter-chunk state):
//   - SPAGHETTI: two decorrelated OpenSimplex fields, carved where BOTH are near
//     zero. Each "near zero" set is a 2-D sheet; their intersection is a 1-D
//     winding curve, i.e. a tunnel. Anisotropic Y keeps tunnels mostly horizontal.
//   - CHEESE: a single low-frequency field dipping below a threshold -> occasional
//     large caverns.
pub const SALT_CAVE_A: u32 = 0xE01;
pub const SALT_CAVE_B: u32 = 0xE02;
pub const SALT_CAVE_C: u32 = 0xE03;
pub const CAVE_FREQ_XZ: f64 = 0.019;
pub const CAVE_FREQ_Y: f64 = 0.028;
/// Tunnel half-thickness in noise units: larger = wider tunnels, more of them.
pub const CAVE_TUNNEL_R: f64 = 0.075;
pub const CAVE_CHEESE_FREQ: f64 = 0.013;
/// Cheese carves where the field drops below this (more negative = rarer rooms).
pub const CAVE_CHEESE_T: f64 = -0.52;
/// Cave carving Y band: keep a solid floor below and solid rock under the surface
/// skin above (no surface holes, no breaking the world floor).
pub const CAVE_MIN_Y: i32 = 5;
pub const CAVE_SURFACE_BUFFER: i32 = 6;
