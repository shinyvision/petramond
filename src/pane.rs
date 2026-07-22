//! Glass-pane helpers that outlive the shared `crate::connect` machinery.
//!
//! A pane keeps no per-cell state — its connection mask + boxes are resolved
//! from the current neighbours by the param-driven `World::connection_*`
//! accessors (shared with fences and every Layer-2 wall/bar). This module now
//! only re-exports the mask bits under their historical `crate::pane::` path and
//! lifts cell-local boxes to world space for the selection outline.

use crate::block::Aabb;
use crate::connect;
use crate::mathh::{IVec3, Vec3, MAX_SELECTION_BOXES};

// The connection-mask bits live in `crate::connect`; re-exported here so the
// many `crate::pane::WEST`-style call sites (the mesher, the world tests) stay
// stable.
pub use crate::connect::{EAST, NORTH, SOUTH, WEST};

/// The engine glass-pane post extent (`7/16..9/16`, `2/16` across) — the shared
/// default a consumer falls back to when the exact shape params are not threaded
/// through (e.g. the break overlay).
pub const POST_LO: f32 = 7.0 / 16.0;
pub const POST_HI: f32 = 9.0 / 16.0;

/// Cell-local boxes lifted to world space for the selection outline (a pane has
/// at most 2 runs, under the outline cap).
#[inline]
pub fn world_boxes(origin: IVec3, boxes: &[Aabb]) -> ([(Vec3, Vec3); MAX_SELECTION_BOXES], u8) {
    connect::world_boxes(origin, boxes)
}
