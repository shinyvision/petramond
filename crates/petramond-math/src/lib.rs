//! Foundation math and wire-format primitives.
//!
//! The lowest layer of the engine: nothing here knows what a block, item, or
//! world is. Everything above may depend on this crate; this crate depends on
//! nothing internal.

pub mod face;
pub mod facing;
pub mod math;
pub mod pose;
pub mod wire_enum;
