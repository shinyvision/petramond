use crate::mathh::{IVec3, Mat4, SelectionBoxes, SelectionShape, Vec3};
use crate::torch::{POLE_HALF, POLE_HEIGHT};

pub(super) const MAX_OUTLINE_VERTICES: usize = 96;

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
        SelectionShape::Boxes { boxes } => box_list_outline_vertices(boxes),
    }
}

/// The 24 line-segment endpoints (12 edges) of a wireframe box. Inflated outward
/// by `INFLATE` so visible front edges sit a hair nearer the camera than the
/// block surface and pass the LessEqual depth test (no z-fighting); back edges
/// remain occluded by the block itself.
fn box_outline_vertices(min: Vec3, max: Vec3) -> OutlineVertices {
    let mut out = OutlineVertices {
        vertices: [[0.0; 3]; MAX_OUTLINE_VERTICES],
        count: 0,
    };
    push_box_edges(&mut out, min, max);
    out
}

fn box_list_outline_vertices(boxes: SelectionBoxes) -> OutlineVertices {
    let mut out = OutlineVertices {
        vertices: [[0.0; 3]; MAX_OUTLINE_VERTICES],
        count: 0,
    };

    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);
    let mut zs = Vec::with_capacity(4);
    for (min, max) in boxes.iter() {
        xs.extend([min.x, max.x]);
        ys.extend([min.y, max.y]);
        zs.extend([min.z, max.z]);
    }
    sort_dedup_coords(&mut xs);
    sort_dedup_coords(&mut ys);
    sort_dedup_coords(&mut zs);
    if xs.len() < 2 || ys.len() < 2 || zs.len() < 2 {
        return out;
    }

    let nx = xs.len() - 1;
    let ny = ys.len() - 1;
    let nz = zs.len() - 1;
    let mut filled = vec![false; nx * ny * nz];
    for iz in 0..nz {
        for iy in 0..ny {
            for ix in 0..nx {
                let p = Vec3::new(
                    (xs[ix] + xs[ix + 1]) * 0.5,
                    (ys[iy] + ys[iy + 1]) * 0.5,
                    (zs[iz] + zs[iz + 1]) * 0.5,
                );
                filled[cell_idx(ix, iy, iz, nx, ny)] =
                    boxes.iter().any(|(min, max)| point_in_box(p, min, max));
            }
        }
    }

    let occupied = |ix: isize, iy: isize, iz: isize| -> bool {
        if ix < 0 || iy < 0 || iz < 0 {
            return false;
        }
        let (ix, iy, iz) = (ix as usize, iy as usize, iz as usize);
        ix < nx && iy < ny && iz < nz && filled[cell_idx(ix, iy, iz, nx, ny)]
    };

    let mut edges = Vec::new();
    for iz in 0..nz {
        for iy in 0..ny {
            for ix in 0..nx {
                if !filled[cell_idx(ix, iy, iz, nx, ny)] {
                    continue;
                }
                let ix = ix as isize;
                let iy = iy as isize;
                let iz = iz as isize;
                let x0 = xs[ix as usize];
                let x1 = xs[ix as usize + 1];
                let y0 = ys[iy as usize];
                let y1 = ys[iy as usize + 1];
                let z0 = zs[iz as usize];
                let z1 = zs[iz as usize + 1];

                if !occupied(ix - 1, iy, iz) {
                    add_face_edges(&mut edges, [-1, 0, 0], face_x(x0, y0, y1, z0, z1));
                }
                if !occupied(ix + 1, iy, iz) {
                    add_face_edges(&mut edges, [1, 0, 0], face_x(x1, y0, y1, z0, z1));
                }
                if !occupied(ix, iy - 1, iz) {
                    add_face_edges(&mut edges, [0, -1, 0], face_y(y0, x0, x1, z0, z1));
                }
                if !occupied(ix, iy + 1, iz) {
                    add_face_edges(&mut edges, [0, 1, 0], face_y(y1, x0, x1, z0, z1));
                }
                if !occupied(ix, iy, iz - 1) {
                    add_face_edges(&mut edges, [0, 0, -1], face_z(z0, x0, x1, y0, y1));
                }
                if !occupied(ix, iy, iz + 1) {
                    add_face_edges(&mut edges, [0, 0, 1], face_z(z1, x0, x1, y0, y1));
                }
            }
        }
    }

    const INFLATE: f32 = 0.003;
    for edge in edges {
        if edge.normal_count < 2 {
            continue;
        }
        let mut offset = [0.0; 3];
        for normal in &edge.normals[..edge.normal_count] {
            offset[0] += normal[0] as f32 * INFLATE;
            offset[1] += normal[1] as f32 * INFLATE;
            offset[2] += normal[2] as f32 * INFLATE;
        }
        push_line(&mut out, add(edge.a, offset), add(edge.b, offset));
    }
    out
}

fn sort_dedup_coords(coords: &mut Vec<f32>) {
    coords.sort_by(|a, b| a.total_cmp(b));
    coords.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-5);
}

fn cell_idx(ix: usize, iy: usize, iz: usize, nx: usize, ny: usize) -> usize {
    (iz * ny + iy) * nx + ix
}

fn point_in_box(p: Vec3, min: Vec3, max: Vec3) -> bool {
    const EPS: f32 = 1.0e-5;
    p.x > min.x + EPS
        && p.x < max.x - EPS
        && p.y > min.y + EPS
        && p.y < max.y - EPS
        && p.z > min.z + EPS
        && p.z < max.z - EPS
}

fn face_x(x: f32, y0: f32, y1: f32, z0: f32, z1: f32) -> [[f32; 3]; 4] {
    [[x, y0, z0], [x, y1, z0], [x, y1, z1], [x, y0, z1]]
}

fn face_y(y: f32, x0: f32, x1: f32, z0: f32, z1: f32) -> [[f32; 3]; 4] {
    [[x0, y, z0], [x1, y, z0], [x1, y, z1], [x0, y, z1]]
}

fn face_z(z: f32, x0: f32, x1: f32, y0: f32, y1: f32) -> [[f32; 3]; 4] {
    [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]]
}

#[derive(Clone)]
struct EdgeRecord {
    key: ([i64; 3], [i64; 3]),
    a: [f32; 3],
    b: [f32; 3],
    normals: [[i8; 3]; 6],
    normal_count: usize,
}

fn add_face_edges(edges: &mut Vec<EdgeRecord>, normal: [i8; 3], corners: [[f32; 3]; 4]) {
    for (a, b) in [
        (corners[0], corners[1]),
        (corners[1], corners[2]),
        (corners[2], corners[3]),
        (corners[3], corners[0]),
    ] {
        add_edge(edges, a, b, normal);
    }
}

fn add_edge(edges: &mut Vec<EdgeRecord>, a: [f32; 3], b: [f32; 3], normal: [i8; 3]) {
    let ka = point_key(a);
    let kb = point_key(b);
    let (key, a, b) = if ka <= kb {
        ((ka, kb), a, b)
    } else {
        ((kb, ka), b, a)
    };
    if let Some(edge) = edges.iter_mut().find(|edge| edge.key == key) {
        if !edge.normals[..edge.normal_count].contains(&normal) {
            debug_assert!(edge.normal_count < edge.normals.len());
            edge.normals[edge.normal_count] = normal;
            edge.normal_count += 1;
        }
        return;
    }
    edges.push(EdgeRecord {
        key,
        a,
        b,
        normals: [
            normal,
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
        ],
        normal_count: 1,
    });
}

fn point_key(p: [f32; 3]) -> [i64; 3] {
    const SCALE: f32 = 1024.0;
    [
        (p[0] * SCALE).round() as i64,
        (p[1] * SCALE).round() as i64,
        (p[2] * SCALE).round() as i64,
    ]
}

fn add(p: [f32; 3], offset: [f32; 3]) -> [f32; 3] {
    [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]
}

fn push_box_edges(out: &mut OutlineVertices, min: Vec3, max: Vec3) {
    const INFLATE: f32 = 0.003;
    let lo = [min.x - INFLATE, min.y - INFLATE, min.z - INFLATE];
    let hi = [max.x + INFLATE, max.y + INFLATE, max.z + INFLATE];
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
    for (a, b) in [
        (c000, c100),
        (c100, c101),
        (c101, c001),
        (c001, c000),
        (c010, c110),
        (c110, c111),
        (c111, c011),
        (c011, c010),
        (c000, c010),
        (c100, c110),
        (c101, c111),
        (c001, c011),
    ] {
        push_line(out, a, b);
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
    let mut out = OutlineVertices {
        vertices: [[0.0; 3]; MAX_OUTLINE_VERTICES],
        count: 0,
    };
    for (a, b) in [
        (c000, c100),
        (c100, c101),
        (c101, c001),
        (c001, c000),
        (c010, c110),
        (c110, c111),
        (c111, c011),
        (c011, c010),
        (c000, c010),
        (c100, c110),
        (c101, c111),
        (c001, c011),
    ] {
        push_line(&mut out, a, b);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facing::Facing;

    #[test]
    fn stair_box_outline_removes_internal_join_but_keeps_step_edges() {
        let (boxes, len) =
            crate::stair::world_boxes(IVec3::ZERO, crate::stair::boxes(Facing::South));
        let outline = outline_vertices(SelectionShape::Boxes {
            boxes: SelectionBoxes { boxes, len },
        });
        let segments = segments(&outline);

        assert!(
            !segments.iter().any(|&(a, b)| {
                mostly_axis(a, b, 1)
                    && near_side_x(mid(a, b)[0])
                    && near(mid(a, b)[2], 0.5, 0.02)
                    && mid(a, b)[1] > -0.02
                    && mid(a, b)[1] < 0.5
            }),
            "the internal lower seam where stair boxes touch should not be outlined"
        );
        assert!(
            segments.iter().any(|&(a, b)| {
                mostly_axis(a, b, 1)
                    && near_side_x(mid(a, b)[0])
                    && near(mid(a, b)[2], 0.5, 0.02)
                    && mid(a, b)[1] > 0.5
            }),
            "the exposed vertical riser edge should still be outlined"
        );
        assert!(
            segments.iter().any(|&(a, b)| {
                mostly_axis(a, b, 0)
                    && near(mid(a, b)[0], 0.5, 0.02)
                    && near(mid(a, b)[1], 0.5, 0.02)
                    && near(mid(a, b)[2], 0.5, 0.02)
            }),
            "the exposed step corner should still be outlined"
        );
    }

    fn segments(outline: &OutlineVertices) -> Vec<([f32; 3], [f32; 3])> {
        outline.vertices[..outline.count as usize]
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .collect()
    }

    fn mostly_axis(a: [f32; 3], b: [f32; 3], axis: usize) -> bool {
        let mut delta = [
            (a[0] - b[0]).abs(),
            (a[1] - b[1]).abs(),
            (a[2] - b[2]).abs(),
        ];
        let along = delta[axis];
        delta[axis] = 0.0;
        along > 0.2 && delta.iter().all(|&d| d < 0.02)
    }

    fn mid(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
        ]
    }

    fn near(v: f32, target: f32, eps: f32) -> bool {
        (v - target).abs() <= eps
    }

    fn near_side_x(x: f32) -> bool {
        near(x, 0.0, 0.02) || near(x, 1.0, 0.02)
    }
}
