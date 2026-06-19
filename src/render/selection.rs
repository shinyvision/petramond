/// The 24 line-segment endpoints (12 edges) of the wireframe cube for block
/// `b`, in world space. Inflated outward by `INFLATE` so visible front edges
/// sit a hair nearer the camera than the block surface and pass the LessEqual
/// depth test (no z-fighting); back edges remain occluded by the block itself.
pub(super) fn outline_vertices(b: glam::IVec3) -> [[f32; 3]; 24] {
    const INFLATE: f32 = 0.003;
    let lo = [
        b.x as f32 - INFLATE,
        b.y as f32 - INFLATE,
        b.z as f32 - INFLATE,
    ];
    let hi = [
        b.x as f32 + 1.0 + INFLATE,
        b.y as f32 + 1.0 + INFLATE,
        b.z as f32 + 1.0 + INFLATE,
    ];
    // 8 corners indexed by (x_hi?, y_hi?, z_hi?).
    let c = |xh: bool, yh: bool, zh: bool| {
        [
            if xh { hi[0] } else { lo[0] },
            if yh { hi[1] } else { lo[1] },
            if zh { hi[2] } else { lo[2] },
        ]
    };
    let c000 = c(false, false, false);
    let c100 = c(true, false, false);
    let c010 = c(false, true, false);
    let c001 = c(false, false, true);
    let c110 = c(true, true, false);
    let c101 = c(true, false, true);
    let c011 = c(false, true, true);
    let c111 = c(true, true, true);
    [
        // bottom rectangle (y = lo)
        c000, c100, c100, c101, c101, c001, c001, c000, // top rectangle (y = hi)
        c010, c110, c110, c111, c111, c011, c011, c010, // four vertical edges
        c000, c010, c100, c110, c101, c111, c001, c011,
    ]
}
