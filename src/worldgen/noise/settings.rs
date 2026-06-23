//! Const cave-sampler settings for [`super::CaveField`].
//!
//! Salts, frequencies, and thresholds are the single source of truth for the
//! cave samplers. Changing any value here changes carved caves by design.
//!
//! Two flavours, both pure 3-D noise functions of world position (so caves are
//! seamless across chunks with no inter-chunk state):
//!   - SPAGHETTI: two decorrelated OpenSimplex fields, carved where BOTH are near
//!     zero. Each "near zero" set is a 2-D sheet; their intersection is a 1-D
//!     winding curve, i.e. a tunnel. Anisotropic Y keeps tunnels mostly horizontal.
//!   - CHEESE: a single low-frequency field dipping below a threshold -> occasional
//!     large caverns.

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
