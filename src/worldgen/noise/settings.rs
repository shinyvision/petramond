//! Const cave-sampler settings for [`super::CaveField`].
//!
//! Salts, frequencies, and thresholds are the single source of truth for the
//! cave samplers. Changing any value here changes carved caves by design.
//!
//! Three flavours, all pure functions of world position (so caves are seamless
//! across chunks/sections with no inter-chunk state):
//!   - SPAGHETTI: two decorrelated OpenSimplex fields, carved where BOTH are near
//!     zero. Each near-zero set is a 2-D sheet; their intersection is a 1-D
//!     winding curve, i.e. a tunnel. Anisotropic Y keeps tunnels mostly horizontal.
//!   - CHEESE: a single low-frequency 3-D field dipping below a threshold creates
//!     occasional larger rooms.
//!   - ENTRANCES: a low-frequency 3-D gate lets the spaghetti field breach the
//!     surface where a real tunnel already approaches it. The mouth is narrow at
//!     the surface and widens downward to normal tunnel thickness.

use crate::chunk::{SEA_LEVEL, SECTION_SIZE, WORLD_MIN_Y};

pub const SALT_CAVE_A: u32 = 0xE01;
pub const SALT_CAVE_B: u32 = 0xE02;
pub const SALT_CAVE_C: u32 = 0xE03;
pub const SALT_CAVE_ROUGHNESS: u32 = 0xE04;
pub const SALT_CAVE_ENTRANCE_A: u32 = 0xE05;
pub const SALT_CAVE_ENTRANCE_B: u32 = 0xE06;
pub const CAVE_FREQ_XZ: f64 = 0.019;
pub const CAVE_FREQ_Y: f64 = 0.028;
/// Tunnel half-thickness in noise units: larger = wider tunnels, more of them.
pub const CAVE_TUNNEL_R: f64 = 0.068;
/// Low-frequency roughness modulates tunnel thickness and cavern rarity.
pub const CAVE_ROUGHNESS_FREQ: f64 = 0.034;
pub const CAVE_TUNNEL_ROUGHNESS: f64 = 0.018;
pub const CAVE_CHEESE_FREQ: f64 = 0.013;
/// Cheese carves where the field drops below this (more negative = rarer rooms).
pub const CAVE_CHEESE_T: f64 = -0.53;
pub const CAVE_CHEESE_ROUGHNESS: f64 = 0.045;
/// Cave carving Y band: keep the bottom two sections solid for broad
/// generated-summary culling and as a floor under the carved cave volume. Except
/// for explicit entrances, keep a solid rock buffer under the surface skin too.
pub const CAVE_MIN_Y: i32 = WORLD_MIN_Y + (SECTION_SIZE as i32 * 2);
pub const CAVE_SURFACE_BUFFER: i32 = 7;

pub const CAVE_ENTRANCE_FREQ: f64 = 0.012;
pub const CAVE_ENTRANCE_Y_SCALE: f64 = 0.55;
pub const CAVE_ENTRANCE_MAX_DEPTH: i32 = 34;
pub const CAVE_ENTRANCE_SURFACE_R: f64 = 0.064;
pub const CAVE_ENTRANCE_DEEP_R: f64 = 0.094;
pub const CAVE_ENTRANCE_GATE_SURFACE_T: f64 = -0.24;
pub const CAVE_ENTRANCE_GATE_DEEP_T: f64 = 0.02;
pub const CAVE_ENTRANCE_MIN_SURFACE_Y: i32 = SEA_LEVEL + 3;
