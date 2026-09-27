use crate::block::Aabb;

pub const MAX_SHAPE_BOXES: usize = 32;

const CELL_MARGIN: f32 = 1.0 / 16.0;

pub fn ingest_shape_boxes(boxes: &[mod_api::ShapeAabb]) -> Result<Vec<Aabb>, String> {
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
