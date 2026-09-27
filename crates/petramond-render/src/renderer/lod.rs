const FAR_LEAF_LOD_FADE_START: f32 = 128.0;
const FAR_LEAF_LOD_FADE_END: f32 = 192.0;
const FAR_LEAF_LOD_HYSTERESIS: f32 = 0.08;

pub(super) fn far_leaf_lod_active(
    dist_sq: f32,
    origin: (i32, i32),
    has_far_lod: bool,
    was_active: bool,
) -> bool {
    if !has_far_lod {
        return false;
    }

    let dist = dist_sq.sqrt();
    if dist <= FAR_LEAF_LOD_FADE_START {
        return false;
    }
    if dist >= FAR_LEAF_LOD_FADE_END {
        return true;
    }

    let t = (dist - FAR_LEAF_LOD_FADE_START) / (FAR_LEAF_LOD_FADE_END - FAR_LEAF_LOD_FADE_START);
    let smooth = t * t * (3.0 - 2.0 * t);
    let threshold = chunk_lod_threshold(origin);
    if was_active {
        smooth + FAR_LEAF_LOD_HYSTERESIS >= threshold
    } else {
        smooth >= threshold + FAR_LEAF_LOD_HYSTERESIS
    }
}

fn chunk_lod_threshold(origin: (i32, i32)) -> f32 {
    let mut h =
        (origin.0 as u32).wrapping_mul(0x9E37_79B1) ^ (origin.1 as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    ((h & 0xFFFF) as f32 + 0.5) / 65_536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_leaf_lod_stays_near_and_converges_far() {
        assert!(!far_leaf_lod_active(200.0 * 200.0, (0, 0), false, false));
        assert!(!far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_START * FAR_LEAF_LOD_FADE_START,
            (0, 0),
            true,
            true
        ));
        assert!(far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_END * FAR_LEAF_LOD_FADE_END,
            (0, 0),
            true,
            false
        ));
    }

    #[test]
    fn far_leaf_lod_transition_is_staggered_by_chunk() {
        let mid = ((FAR_LEAF_LOD_FADE_START + FAR_LEAF_LOD_FADE_END) * 0.5).powi(2);
        let mut near_count = 0;
        let mut far_count = 0;
        for z in -8..=8 {
            for x in -8..=8 {
                if far_leaf_lod_active(mid, (x * 16, z * 16), true, false) {
                    far_count += 1;
                } else {
                    near_count += 1;
                }
            }
        }

        assert!(near_count > 0);
        assert!(far_count > 0);
    }

    #[test]
    fn far_leaf_lod_has_sticky_transition() {
        let origin = (0, 0);
        for i in 1..1000 {
            let t = i as f32 / 1000.0;
            let dist =
                FAR_LEAF_LOD_FADE_START + t * (FAR_LEAF_LOD_FADE_END - FAR_LEAF_LOD_FADE_START);
            let dist_sq = dist * dist;
            if far_leaf_lod_active(dist_sq, origin, true, true)
                && !far_leaf_lod_active(dist_sq, origin, true, false)
            {
                return;
            }
        }
        panic!("expected at least one distance where prior state determines the LOD");
    }
}
