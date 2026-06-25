use crate::mathh::{IVec3, Mat4, SelectionShape, Vec3};
use crate::torch::{POLE_HALF, POLE_HEIGHT};

pub(super) const MAX_OUTLINE_VERTICES: usize = 24;

pub(super) struct OutlineVertices {
    pub vertices: [[f32; 3]; MAX_OUTLINE_VERTICES],
    pub count: u32,
}

/// The line-segment endpoints for a selection outline, in world space.
pub(super) fn outline_vertices(shape: SelectionShape) -> OutlineVertices {
    match shape {
        SelectionShape::Box { min, max } => box_outline_vertices(min, max),
        SelectionShape::Cross {
            origin,
            u_min,
            u_max,
            v_min,
            v_max,
        } => cross_outline_vertices(origin, u_min, u_max, v_min, v_max),
        SelectionShape::Torch { origin, transform } => torch_outline_vertices(origin, transform),
    }
}

/// The 24 line-segment endpoints (12 edges) of a wireframe box. Inflated outward
/// by `INFLATE` so visible front edges sit a hair nearer the camera than the
/// block surface and pass the LessEqual depth test (no z-fighting); back edges
/// remain occluded by the block itself.
fn box_outline_vertices(min: Vec3, max: Vec3) -> OutlineVertices {
    const INFLATE: f32 = 0.003;
    let lo = [min.x - INFLATE, min.y - INFLATE, min.z - INFLATE];
    let hi = [max.x + INFLATE, max.y + INFLATE, max.z + INFLATE];
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
    let vertices = [
        // bottom rectangle (y = lo)
        c000, c100, c100, c101, c101, c001, c001, c000, // top rectangle (y = hi)
        c010, c110, c110, c111, c111, c011, c011, c010, // four vertical edges
        c000, c010, c100, c110, c101, c111, c001, c011,
    ];
    OutlineVertices {
        vertices,
        count: MAX_OUTLINE_VERTICES as u32,
    }
}

/// The 12 edges of the torch's pole box, `transform`-mapped from local model space
/// and offset by the cell `origin`. Mirrors [`box_outline_vertices`]'s edge layout
/// but over a (possibly tilted) box, so a floor torch outlines a straight pole and a
/// wall torch a leaning one — matching `mesh::torch`, which uses the same transform.
fn torch_outline_vertices(origin: IVec3, transform: Mat4) -> OutlineVertices {
    // Inflate in the torch's LOCAL frame so the wireframe sits a hair outside the
    // pole on every face after the tilt (same purpose as box `INFLATE`).
    const INFLATE: f32 = 0.003;
    let lo = [-POLE_HALF - INFLATE, -INFLATE, -POLE_HALF - INFLATE];
    let hi = [
        POLE_HALF + INFLATE,
        POLE_HEIGHT + INFLATE,
        POLE_HALF + INFLATE,
    ];
    let base = Vec3::new(origin.x as f32, origin.y as f32, origin.z as f32);
    // World-space corner for (x_hi?, y_hi?, z_hi?), transformed then cell-offset.
    let c = |xh: bool, yh: bool, zh: bool| {
        let local = Vec3::new(
            if xh { hi[0] } else { lo[0] },
            if yh { hi[1] } else { lo[1] },
            if zh { hi[2] } else { lo[2] },
        );
        let p = base + transform.transform_point3(local);
        [p.x, p.y, p.z]
    };
    let c000 = c(false, false, false);
    let c100 = c(true, false, false);
    let c010 = c(false, true, false);
    let c001 = c(false, false, true);
    let c110 = c(true, true, false);
    let c101 = c(true, false, true);
    let c011 = c(false, true, true);
    let c111 = c(true, true, true);
    let vertices = [
        // bottom rectangle (local y = lo)
        c000, c100, c100, c101, c101, c001, c001, c000, // top rectangle (local y = hi)
        c010, c110, c110, c111, c111, c011, c011, c010, // four vertical edges
        c000, c010, c100, c110, c101, c111, c001, c011,
    ];
    OutlineVertices {
        vertices,
        count: MAX_OUTLINE_VERTICES as u32,
    }
}

fn cross_outline_vertices(
    origin: IVec3,
    u_min: f32,
    u_max: f32,
    v_min: f32,
    v_max: f32,
) -> OutlineVertices {
    const PAD: f32 = 0.002;
    let u0 = (u_min - PAD).clamp(0.0, 1.0);
    let u1 = (u_max + PAD).clamp(0.0, 1.0);
    let v0 = (v_min - PAD).clamp(0.0, 1.0);
    let v1 = (v_max + PAD).clamp(0.0, 1.0);

    let x = origin.x as f32;
    let y = origin.y as f32;
    let z = origin.z as f32;
    let mut out = OutlineVertices {
        vertices: [[0.0; 3]; MAX_OUTLINE_VERTICES],
        count: 0,
    };

    let p0 = |u: f32, v: f32| [x + u, y + v, z + u];
    push_rect(&mut out, p0(u0, v0), p0(u1, v0), p0(u1, v1), p0(u0, v1));

    let p1 = |u: f32, v: f32| [x + u, y + v, z + 1.0 - u];
    push_rect(&mut out, p1(u0, v0), p1(u1, v0), p1(u1, v1), p1(u0, v1));

    out
}

fn push_rect(out: &mut OutlineVertices, p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3]) {
    push_line(out, p0, p1);
    push_line(out, p1, p2);
    push_line(out, p2, p3);
    push_line(out, p3, p0);
}

fn push_line(out: &mut OutlineVertices, a: [f32; 3], b: [f32; 3]) {
    let i = out.count as usize;
    debug_assert!(i + 2 <= MAX_OUTLINE_VERTICES);
    out.vertices[i] = a;
    out.vertices[i + 1] = b;
    out.count += 2;
}
