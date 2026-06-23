//! Far-chunk leaf level-of-detail selection. A DIFFERENT concern from section
//! culling (which decides *whether* a section is drawn): this decides *which mesh
//! variant* (full vs leaf-decimated "far" mesh) a distant chunk draws, with a
//! per-chunk staggered fade so the swap doesn't pop across a hard distance ring.

/// Distance (world units) at which the far-leaf LOD fade begins. Closer than
/// this a chunk always draws its full mesh.
const FAR_LEAF_LOD_FADE_START: f32 = 128.0;
/// Distance (world units) at which every chunk with a far mesh has switched to it.
const FAR_LEAF_LOD_FADE_END: f32 = 192.0;

/// Should this chunk draw its decimated "far leaf" opaque mesh this frame?
///
/// `false` (full mesh) when the chunk has no far mesh, or is nearer than the fade
/// start; `true` (far mesh) beyond the fade end. In the fade band the smoothstep
/// of the normalized distance is compared against a per-chunk threshold
/// ([`chunk_lod_threshold`]) so chunks cross over at staggered distances rather
/// than all snapping at one ring.
pub(super) fn far_leaf_lod_active(dist_sq: f32, origin: (i32, i32), has_far_lod: bool) -> bool {
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
    smooth >= chunk_lod_threshold(origin)
}

/// A stable per-chunk threshold in `[0, 1)` (hashed from the chunk origin) that
/// staggers the LOD crossover so neighbours don't pop together.
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
        assert!(!far_leaf_lod_active(200.0 * 200.0, (0, 0), false));
        assert!(!far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_START * FAR_LEAF_LOD_FADE_START,
            (0, 0),
            true
        ));
        assert!(far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_END * FAR_LEAF_LOD_FADE_END,
            (0, 0),
            true
        ));
    }

    #[test]
    fn far_leaf_lod_transition_is_staggered_by_chunk() {
        let mid = ((FAR_LEAF_LOD_FADE_START + FAR_LEAF_LOD_FADE_END) * 0.5).powi(2);
        let mut near_count = 0;
        let mut far_count = 0;
        for z in -8..=8 {
            for x in -8..=8 {
                if far_leaf_lod_active(mid, (x * 16, z * 16), true) {
                    far_count += 1;
                } else {
                    near_count += 1;
                }
            }
        }

        assert!(near_count > 0);
        assert!(far_count > 0);
    }
}
