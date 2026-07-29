//! Shared plumbing for the WASM shape bake pumps (server tick + client
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

/// The wire input for one dirty cell — built once by
/// `World::bake_cell_input` when the cell is drained, so every dispatcher
/// ships the identical input.
pub(in crate::modding) fn cell_input(c: &CustomBakeCell) -> mod_api::CellInput {
    c.input.clone()
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

/// Validate a RENDER bake reply into the drawn boxes per cell. The geometry
/// runs the shared box sanitation; the per-box tint needs none (any unorm8
/// triple is a valid multiply, `None` = untinted white).
pub(in crate::modding) fn ingest_render_bake(
    baked: &[mod_api::BakedRenderCell],
    expected: usize,
) -> BakeIngest<Box<[crate::block::ShapeRenderBox]>> {
    ingest(baked, expected, |c| {
        let aabbs: Vec<mod_api::ShapeAabb> = c.boxes.iter().map(|b| b.aabb).collect();
        crate::world::ingest_shape_boxes(&aabbs)
            .map(|clean| {
                clean
                    .into_iter()
                    .zip(&c.boxes)
                    .map(|(aabb, b)| crate::block::ShapeRenderBox {
                        aabb,
                        tint: b.tint.map_or([1.0; 3], |t| t.map(|v| f32::from(v) / 255.0)),
                        ao_strength: b.ao.map_or(1.0, |p| (f32::from(p) / 100.0).clamp(0.0, 1.0)),
                        dyed: b.dyed,
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            })
            .map_err(|e| format!("shape render bake {e}"))
    })
}
