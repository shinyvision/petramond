//! Glass-pane helpers that outlive the shared `crate::connect` machinery.
//!
//! A pane keeps no per-cell state — its connection mask + boxes are resolved
//! from the current neighbours by the param-driven `World::connection_*`
//! accessors (shared with fences and every parameterized wall/bar). This module now
//! only re-exports the mask bits under their historical `crate::pane::` path and
//! lifts cell-local boxes to world space for the selection outline.

// The connection-mask bits live in `crate::connect`; re-exported here so the
// many `crate::pane::WEST`-style call sites (the mesher, the world tests) stay
// stable.
pub use crate::connect::{EAST, NORTH, SOUTH, WEST};
