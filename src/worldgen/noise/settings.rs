//! Const cave-sampler settings for [`super::CaveField`].
//!
//! Salts, frequencies, and thresholds are the single source of truth for the
//! cave samplers. Changing any value here changes carved caves by design.
//!
//! Four carvers plus a biome field, all pure functions of world position (so
//! caves are seamless across chunks/sections with no inter-chunk state):
//!   - SPAGHETTI: two decorrelated OpenSimplex fields, carved where BOTH are near
//!     zero. Each near-zero set is a 2-D sheet; their intersection is a 1-D
//!     winding curve, i.e. a tunnel. Low frequency = long, sweeping tunnels; a
//!     slow roughness field fattens and pinches them along their run.
//!     A second BRANCH system shares field A with the main system,
//!     so both tunnel families run along the same A≈0 sheet and genuinely cross
//!     at isolated points — natural junctions, tunnels that fork every now and
//!     then. Widths are calibrated against the noodle caliber (radius/frequency):
//!     normally ~4× a noodle, swelling to ~6× at the thickness peak.
//!   - NOODLE: the same construction at ~2.5× the frequency with a much smaller
//!     radius — tight, twisting 1–2 block crawl spaces. Gated to the LOW side of
//!     the roughness field, so crawl mazes appear where the spaghetti runs thin
//!     (and stay out of the fat-tunnel regions).
//!   - CHEESE: a single low-frequency 3-D field dipping below a threshold creates
//!     occasional large caverns. The threshold is DEPTH-SCALED: caverns are rare
//!     near the surface and grow common (and huge) toward the world floor.
//!   - ENTRANCES: a low-frequency 3-D gate lets the spaghetti field breach the
//!     surface where a real tunnel already approaches it. The mouth is narrow at
//!     the surface and widens downward to normal tunnel thickness.
//!   - CAVE BIOME: a very-low-frequency 3-D field partitions the underground into
//!     cave biomes (regular stone vs. marble). Marble caves line every carved
//!     surface with a shell of marble (see `CAVE_LINING_SHELL`).

use crate::chunk::{SEA_LEVEL, SECTION_SIZE, WORLD_MIN_Y};

pub const SALT_CAVE_A: u32 = 0xE01;
pub const SALT_CAVE_B: u32 = 0xE02;
pub const SALT_CAVE_C: u32 = 0xE03;
pub const SALT_CAVE_ROUGHNESS: u32 = 0xE04;
pub const SALT_CAVE_ENTRANCE_A: u32 = 0xE05;
pub const SALT_CAVE_ENTRANCE_B: u32 = 0xE06;
pub const SALT_CAVE_NOODLE_A: u32 = 0xE07;
pub const SALT_CAVE_NOODLE_B: u32 = 0xE08;
pub const SALT_CAVE_BIOME: u32 = 0xE09;
pub const SALT_CAVE_BRANCH: u32 = 0xE0A;

// --- Spaghetti ----------------------------------------------------------------
/// Low frequency = long sweeping tunnels with ~90-block winding wavelength.
pub const CAVE_FREQ_XZ: f64 = 0.011;
pub const CAVE_FREQ_Y: f64 = 0.017;
/// Tunnel half-thickness in noise units. Tunnel caliber in blocks scales as
/// radius/frequency, so against the noodle caliber (0.025/0.027 ≈ 0.93) this
/// base is exactly 4× a noodle (0.041/0.011 ≈ 3.7 ≈ 4 × 0.93); the thickness
/// modulation below swells it to exactly 6× at its peak (0.061/0.011).
pub const CAVE_TUNNEL_R: f64 = 0.041;
/// Slow roughness field modulates tunnel thickness (fat halls / tight pinches),
/// gates noodles, and nudges cavern rarity. ±0.53 noise × this coefficient
/// spans the 2×..6× noodle-width band around the 4× base.
pub const CAVE_ROUGHNESS_FREQ: f64 = 0.016;
pub const CAVE_TUNNEL_ROUGHNESS: f64 = 0.038;
/// The BRANCH tunnel system's radius, as a fraction of the main system's
/// (post-modulation): side passages read slightly tighter than the main run.
pub const CAVE_BRANCH_R_SCALE: f64 = 0.85;

// --- Noodle -------------------------------------------------------------------
/// Higher frequency = twistier, shorter-wavelength crawl tunnels.
pub const CAVE_NOODLE_FREQ_XZ: f64 = 0.027;
pub const CAVE_NOODLE_FREQ_Y: f64 = 0.039;
/// Very thin: 1–2 blocks wide at these gradients.
pub const CAVE_NOODLE_R: f64 = 0.025;
/// Noodles exist only where the roughness field is BELOW this (regional
/// patches): crawl mazes fill the thin-spaghetti regions, not the fat ones.
pub const CAVE_NOODLE_GATE_T: f64 = 0.0;

// --- Cheese -------------------------------------------------------------------
pub const CAVE_CHEESE_FREQ: f64 = 0.0082;
/// Depth-scaled carve threshold (more negative = rarer rooms): caverns are rare
/// near/above sea level and grow common toward the world floor.
pub const CAVE_CHEESE_T_SHALLOW: f64 = -0.47;
pub const CAVE_CHEESE_T_DEEP: f64 = -0.36;
/// The threshold ramps from SHALLOW at/above this Y ...
pub const CAVE_CHEESE_DEPTH_TOP: i32 = 48;
/// ... to DEEP at/below this Y.
pub const CAVE_CHEESE_DEPTH_BOTTOM: i32 = -40;
pub const CAVE_CHEESE_ROUGHNESS: f64 = 0.045;

/// Cave carving Y band: keep the bottom section solid for broad
/// generated-summary culling and as a floor under the carved cave volume. Except
/// for explicit entrances, keep a solid rock buffer under the surface skin too.
pub const CAVE_MIN_Y: i32 = WORLD_MIN_Y + SECTION_SIZE as i32;
pub const CAVE_SURFACE_BUFFER: i32 = 7;

// --- Entrances ------------------------------------------------------------------
pub const CAVE_ENTRANCE_FREQ: f64 = 0.012;
pub const CAVE_ENTRANCE_Y_SCALE: f64 = 0.55;
pub const CAVE_ENTRANCE_MAX_DEPTH: i32 = 34;
pub const CAVE_ENTRANCE_SURFACE_R: f64 = 0.078;
pub const CAVE_ENTRANCE_DEEP_R: f64 = 0.100;
pub const CAVE_ENTRANCE_GATE_SURFACE_T: f64 = 0.0;
pub const CAVE_ENTRANCE_GATE_DEEP_T: f64 = 0.02;
pub const CAVE_ENTRANCE_MIN_SURFACE_Y: i32 = SEA_LEVEL + 3;

// --- Cave biomes -----------------------------------------------------------------
/// Very low frequency: cave biome regions span a few hundred blocks, taller than
/// wide so one biome tends to own a whole vertical cave system.
pub const CAVE_BIOME_FREQ: f64 = 0.0045;
pub const CAVE_BIOME_FREQ_Y: f64 = 0.0027;
/// Marble caves where the biome field exceeds this (~1/5 of the underground).
pub const CAVE_BIOME_MARBLE_T: f64 = 0.17;
/// Wall-lining shell width, in noise units ABOVE each carver's carve threshold: a
/// solid voxel whose carve metric is within the shell of the threshold hugs the
/// cave wall, so a marble cave paints it marble. ~1–2 blocks at these gradients.
pub const CAVE_LINING_SHELL: f64 = 0.022;
/// Cheese has a shallower gradient, so its shell is separate (~2–3 blocks).
pub const CAVE_CHEESE_LINING_SHELL: f64 = 0.030;
