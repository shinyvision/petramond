//! `noise` subsystem — the live cave field over typed `noise`-crate samplers.
//!
//! The legacy `WorldNoise`/`HeightField` terrain generator was excised; active
//! chunk terrain uses the classic biome terrain provider plus `worldgen::river`
//! for explicit river carving. What remains here is [`CaveField`], the 3-D cave
//! carve used during column fill, plus its sampler [`settings`].

pub mod height;
pub mod settings;

pub use height::CaveField;
