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
pub const RIVER_OCTAVES: usize = 2;
pub const RIVER_FREQ: f64 = 0.000_65;
