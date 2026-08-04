//! Box-set mesh geometry for the parameterized shape families (fences, panes,
//! ladders, slabs): pure geometry over shape state and tiles — no GPU types.
//! The chunk mesher (engine crate) consumes these through its historical
//! `mesh::{fence,pane,ladder,slab}` paths.

pub mod fence;
pub mod ladder;
pub mod pane;
pub mod slab;
