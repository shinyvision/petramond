//! Shared ingest validation for Layer-3 guest bake geometry — the ONE place a
//! `ShapeAabb` reply crosses into engine `Aabb`s the physics sweeps, the light
//! flood reads, and the mesher emits. Guest replies are hostile input: a bake
//! can return NaN boxes (every physics overlap goes false — the player falls
//! through the shape), inverted boxes, kilometre-long boxes (stray quads until
//! the vertex clamp), or an unbounded count. Both bake pumps (server + client)
//! and the item bake run every box through [`ingest_shape_boxes`] before it
//! reaches any cache.

use crate::block::Aabb;

/// Per-cell box-count cap — the `SIM_BATCH_MAX` doctrine applied to bake output.
/// Voxel furniture peaks in the single digits; 32 is orders of magnitude above
/// legitimate use while a maximal box list stays microseconds of ingest work.
pub(crate) const MAX_SHAPE_BOXES: usize = 32;

/// How far outside the unit cell a baked box may reach before it is clamped
/// back in (one texel of margin, matching the render/collision conventions).
const CELL_MARGIN: f32 = 1.0 / 16.0;

/// Validate + normalize one cell's baked boxes into engine `Aabb`s.
///
/// `Err(reason)` is a PROTOCOL BREAK (disable the mod, exactly like a
/// wrong-nonzero-length reply): the count exceeds [`MAX_SHAPE_BOXES`], a
/// component is non-finite, or a box is inverted (`min > max` on some axis). A
/// box that merely escapes the cell is CLAMPED to `[-1/16, 17/16]` (benign
/// over-reach, not a break). An empty list is fine (the caller treats it as
/// "no bake, use fallback").
pub(crate) fn ingest_shape_boxes(boxes: &[mod_api::ShapeAabb]) -> Result<Vec<Aabb>, String> {
    if boxes.len() > MAX_SHAPE_BOXES {
        return Err(format!(
            "shape bake returned {} boxes (max {MAX_SHAPE_BOXES})",
            boxes.len()
        ));
    }
    let lo = -CELL_MARGIN;
    let hi = 1.0 + CELL_MARGIN;
    let mut out = Vec::with_capacity(boxes.len());
    for b in boxes {
        if !b.min.iter().chain(b.max.iter()).all(|c| c.is_finite()) {
            return Err("shape bake box has a non-finite component".into());
        }
        if (0..3).any(|a| b.min[a] > b.max[a]) {
            return Err("shape bake box is inverted (min > max)".into());
        }
        // Clamp is monotonic, so `min <= max` still holds after it.
        let clamp = |v: f32| v.clamp(lo, hi);
        out.push(Aabb {
            min: [clamp(b.min[0]), clamp(b.min[1]), clamp(b.min[2])],
            max: [clamp(b.max[0]), clamp(b.max[1]), clamp(b.max[2])],
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> mod_api::ShapeAabb {
        mod_api::ShapeAabb { min, max }
    }

    #[test]
    fn accepts_and_clamps_in_range_and_over_reach() {
        let ok = ingest_shape_boxes(&[aabb([0.0, 0.0, 0.0], [1.0, 0.5, 1.0])]).unwrap();
        assert_eq!(ok.len(), 1);
        // An over-reaching box is clamped to the cell ± one texel, not rejected.
        let clamped = ingest_shape_boxes(&[aabb([-5.0, 0.0, 0.0], [1.0, 9000.0, 1.0])]).unwrap();
        assert_eq!(clamped[0].min[0], -1.0 / 16.0);
        assert_eq!(clamped[0].max[1], 1.0 + 1.0 / 16.0);
    }

    #[test]
    fn rejects_nonfinite_inverted_and_overcount() {
        assert!(ingest_shape_boxes(&[aabb([f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0])]).is_err());
        assert!(ingest_shape_boxes(&[aabb([0.0, 0.0, 0.0], [f32::INFINITY, 1.0, 1.0])]).is_err());
        assert!(ingest_shape_boxes(&[aabb([0.6, 0.0, 0.0], [0.4, 1.0, 1.0])]).is_err());
        let too_many = vec![aabb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]); MAX_SHAPE_BOXES + 1];
        assert!(ingest_shape_boxes(&too_many).is_err());
    }
}
