//! Cave-oriented noise subsystem over typed `noise`-crate samplers.
//!
//! Caves are an explicit post-surface carve stage, not nodes in the surface
//! density graph. The samplers stay pure world-position functions so chunk and
//! section generation remain order-independent.

pub mod height;
pub mod settings;
