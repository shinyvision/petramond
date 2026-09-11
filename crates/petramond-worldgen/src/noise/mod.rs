//! Independent cave density, branching walks and bounded excavations.
//!
//! Caves are an explicit post-surface carve stage, not nodes in the surface
//! density graph. The samplers stay pure world-position functions so chunk and
//! section generation remain order-independent.

mod cave_density;
pub mod cave_field;
mod cave_walk;
mod chamber;
pub mod settings;
