//! Shared plumbing for the Layer-3 shape bake pumps (server tick + client
//! replica). Both build the same [`mod_api::CellInput`] batch, and both gate
//! the reply through the SAME failure policy (M5) and geometry sanitation (C2):
//!
//! - an EMPTY reply means "no bake, use the static fallback" (a shape that
//!   declines to bake, or a `client_wasm` that implements only the other side);
//! - a wrong-but-NONZERO length is a protocol break that disables the mod;
//! - a correct-length reply has every cell's boxes validated
//!   ([`crate::world::ingest_shape_boxes`]) before anything reaches a cache — a
//!   non-finite / inverted / over-count box disables the mod too.
//!
//! Keeping this in one place is what makes the server and client pumps behave
//! identically, so a shape cannot pass on one side and desync on the other.

use crate::block::Aabb;
use crate::world::CustomBakeCell;

/// The wire input for one dirty cell — the ENTIRE bake input (block + the six
/// neighbour ids); there is no per-cell state on the wire.
pub(in crate::modding) fn cell_input(c: &CustomBakeCell) -> mod_api::CellInput {
    mod_api::CellInput {
        world_pos: [c.pos.x, c.pos.y, c.pos.z],
        block_id: mod_api::BlockId(c.block_id),
        neighbor_ids: c.neighbor_ids.map(mod_api::BlockId),
    }
}

/// The verdict on a batch bake reply.
pub(in crate::modding) enum BakeIngest<T> {
    /// The validated per-cell geometry, ready to cache (in input-cell order).
    Apply(Vec<T>),
    /// No bake — leave the cells on their static fallback (empty reply / a
    /// disabled or unreachable mod).
    Fallback,
    /// A protocol break: disable the mod with this reason.
    Disable(String),
}

/// The shared reply gate (M5) around a per-cell mapper (C2).
fn ingest<C, T>(
    baked: &[C],
    expected: usize,
    mut per_cell: impl FnMut(&C) -> Result<T, String>,
) -> BakeIngest<T> {
    if baked.is_empty() {
        return BakeIngest::Fallback;
    }
    if baked.len() != expected {
        return BakeIngest::Disable("bake returned the wrong cell count".into());
    }
    let mut out = Vec::with_capacity(baked.len());
    for c in baked {
        match per_cell(c) {
            Ok(v) => out.push(v),
            Err(reason) => return BakeIngest::Disable(reason),
        }
    }
    BakeIngest::Apply(out)
}

/// Validate a SIM bake reply into `(collision boxes, light aperture)` per cell.
pub(in crate::modding) fn ingest_sim_bake(
    baked: &[mod_api::BakedSimCell],
    expected: usize,
) -> BakeIngest<(Vec<Aabb>, mod_api::LightAperture)> {
    ingest(baked, expected, |c| {
        crate::world::ingest_shape_boxes(&c.collision_boxes)
            .map(|boxes| (boxes, c.light_aperture))
            .map_err(|e| format!("shape sim bake {e}"))
    })
}

/// Validate a RENDER bake reply into the drawn boxes per cell.
pub(in crate::modding) fn ingest_render_bake(
    baked: &[mod_api::BakedRenderCell],
    expected: usize,
) -> BakeIngest<Box<[Aabb]>> {
    ingest(baked, expected, |c| {
        crate::world::ingest_shape_boxes(&c.boxes)
            .map(Vec::into_boxed_slice)
            .map_err(|e| format!("shape render bake {e}"))
    })
}
