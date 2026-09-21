use super::{Selection, SelectionBox};
use petramond_math::{math::Vec3, world_pos::WorldPos};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceRect {
    pub lo: [i32; 2],
    pub hi: [i32; 2],
}

impl FaceRect {
    fn subtract(self, other: Self, out: &mut Vec<Self>) {
        let lo = std::array::from_fn::<_, 2, _>(|i| self.lo[i].max(other.lo[i]));
        let hi = std::array::from_fn::<_, 2, _>(|i| self.hi[i].min(other.hi[i]));
        if (0..2).any(|i| lo[i] >= hi[i]) {
            out.push(self);
            return;
        }
        let mut core = self;
        for axis in 0..2 {
            if core.lo[axis] < lo[axis] {
                let mut part = core;
                part.hi[axis] = lo[axis];
                out.push(part);
                core.lo[axis] = lo[axis];
            }
            if hi[axis] < core.hi[axis] {
                let mut part = core;
                part.lo[axis] = hi[axis];
                out.push(part);
                core.hi[axis] = hi[axis];
            }
        }
    }

    fn touches(self, other: Self) -> bool {
        (0..2).any(|axis| {
            let tangent = 1 - axis;
            (self.hi[axis] == other.lo[axis] || other.hi[axis] == self.lo[axis])
                && self.lo[tangent].max(other.lo[tangent]) < self.hi[tangent].min(other.hi[tangent])
        })
    }
}

/// One connected, coplanar piece of the selection boundary, including its holes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionFace {
    pub axis: usize,
    pub normal: i32,
    pub plane: i32,
    pub rectangles: Vec<FaceRect>,
}

impl SelectionFace {
    pub fn quads(&self) -> impl Iterator<Item = [[i32; 3]; 4]> + '_ {
        self.rectangles.iter().map(|r| {
            [r.lo, [r.hi[0], r.lo[1]], r.hi, [r.lo[0], r.hi[1]]].map(|uv| {
                let mut p = [0; 3];
                p[self.axis] = self.plane;
                p[(self.axis + 1) % 3] = uv[0];
                p[(self.axis + 2) % 3] = uv[1];
                p
            })
        })
    }

    pub(super) fn sweep(&self, plane: i32) -> impl Iterator<Item = SelectionBox> + '_ {
        self.rectangles.iter().map(move |r| {
            let mut lo = [0; 3];
            let mut hi = [0; 3];
            lo[self.axis] = self.plane.min(plane);
            hi[self.axis] = self.plane.max(plane);
            for i in 0..2 {
                lo[(self.axis + i + 1) % 3] = r.lo[i];
                hi[(self.axis + i + 1) % 3] = r.hi[i];
            }
            SelectionBox { lo, hi }
        })
    }
}

/// Cached boundary geometry; its cost depends on box fragmentation, never volume.
#[derive(Default)]
pub struct SelectionSurface {
    pub(super) faces: Vec<SelectionFace>,
}

impl SelectionSurface {
    pub fn faces(&self) -> &[SelectionFace] {
        &self.faces
    }

    pub fn pick(&self, eye: WorldPos, dir: Vec3, reach: f32) -> Option<(&SelectionFace, f64)> {
        if !eye.is_finite() || !dir.is_finite() || !reach.is_finite() || reach < 0.0 {
            return None;
        }
        let dir = dir.normalize_or_zero();
        let eye = [eye.x, eye.y, eye.z];
        self.faces
            .iter()
            .filter_map(|face| {
                let axis = face.axis;
                if dir[axis].abs() < 1e-7 {
                    return None;
                }
                let distance = (f64::from(face.plane) - eye[axis]) / f64::from(dir[axis]);
                if distance < 0.0 || distance > f64::from(reach) {
                    return None;
                }
                let uv: [f64; 2] = std::array::from_fn(|i| {
                    let a = (axis + i + 1) % 3;
                    eye[a] + distance * f64::from(dir[a])
                });
                face.rectangles
                    .iter()
                    .any(|r| {
                        (0..2).all(|i| uv[i] >= f64::from(r.lo[i]) && uv[i] <= f64::from(r.hi[i]))
                    })
                    .then_some((face, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }
}

impl Selection {
    pub fn surface(&self) -> SelectionSurface {
        let mut planes = BTreeMap::<(usize, i32), [Vec<FaceRect>; 2]>::new();
        for region in self.regions() {
            for axis in 0..3 {
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                let rect = FaceRect {
                    lo: [region.lo[u], region.lo[v]],
                    hi: [region.hi[u], region.hi[v]],
                };
                planes.entry((axis, region.lo[axis])).or_default()[0].push(rect);
                planes.entry((axis, region.hi[axis])).or_default()[1].push(rect);
            }
        }
        let mut faces = Vec::new();
        for ((axis, plane), sides) in planes {
            for side in 0..2 {
                let mut exposed = Vec::new();
                for rect in &sides[side] {
                    let mut pieces = vec![*rect];
                    for opposite in &sides[1 - side] {
                        let mut remaining = Vec::new();
                        for piece in pieces {
                            piece.subtract(*opposite, &mut remaining);
                        }
                        pieces = remaining;
                        if pieces.is_empty() {
                            break;
                        }
                    }
                    exposed.extend(pieces);
                }
                while let Some(first) = exposed.pop() {
                    let mut rectangles = vec![first];
                    let mut cursor = 0;
                    while cursor < rectangles.len() {
                        let mut i = 0;
                        while i < exposed.len() {
                            if rectangles[cursor].touches(exposed[i]) {
                                rectangles.push(exposed.swap_remove(i));
                            } else {
                                i += 1;
                            }
                        }
                        cursor += 1;
                    }
                    faces.push(SelectionFace {
                        axis,
                        normal: if side == 0 { -1 } else { 1 },
                        plane,
                        rectangles,
                    });
                }
            }
        }
        SelectionSurface { faces }
    }
}
