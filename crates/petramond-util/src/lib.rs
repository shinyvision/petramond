//! Cross-cutting utilities with no game-domain knowledge.
//!
//! `test_time` is always compiled (it is a few lines) so downstream crates can
//! use it from `#[cfg(test)]` code without a dev-dependency cycle.

pub mod bytecodec;
pub mod memory;
pub mod paths;
pub mod test_time;
pub mod texture_mips;
