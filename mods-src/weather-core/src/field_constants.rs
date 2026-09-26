/// Field period in blocks. Power of two; all octave lattices tile at it.
pub const WRAP: f32 = 65536.0;
/// Base octave feature size in blocks. Power of two dividing [`WRAP`].
pub const FEATURE_SIZE: f32 = 512.0;
/// The second cloud population has larger, independently seeded features.
pub const SHEET_B_FEATURE: f32 = 1024.0;
/// Integer advection multiple, keeping the sheet wrap-exact.
pub const SHEET_B_ADVECT: f32 = 2.0;
/// Seed salt separating sheet B's hash stream from sheet A's.
pub const SHEET_B_SALT: u32 = 0x517C_C1B7;
/// Fraction of the post-threshold coverage range used for the rain ramp.
pub const RAIN_RAMP: f32 = 0.6;
