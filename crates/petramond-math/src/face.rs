use glam::IVec3;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Face {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::PosX,
        Face::NegX,
        Face::PosY,
        Face::NegY,
        Face::PosZ,
        Face::NegZ,
    ];

    #[inline]
    pub fn dir(self) -> IVec3 {
        crate::math::FACE_NEIGHBORS[self as usize]
    }

    pub fn ao_u(self) -> IVec3 {
        match self {
            Face::PosX | Face::NegX => IVec3::Y,
            Face::PosY | Face::NegY => IVec3::X,
            Face::PosZ | Face::NegZ => IVec3::X,
        }
    }

    pub fn ao_v(self) -> IVec3 {
        match self {
            Face::PosX | Face::NegX => IVec3::Z,
            Face::PosY | Face::NegY => IVec3::Z,
            Face::PosZ | Face::NegZ => IVec3::Y,
        }
    }

    pub fn ao_signs(self) -> [(i32, i32); 4] {
        match self {
            Face::PosX => [(-1, 1), (-1, -1), (1, -1), (1, 1)],
            Face::NegX => [(-1, -1), (-1, 1), (1, 1), (1, -1)],
            Face::PosY => [(-1, 1), (1, 1), (1, -1), (-1, -1)],
            Face::NegY => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
            Face::PosZ => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
            Face::NegZ => [(1, -1), (-1, -1), (-1, 1), (1, 1)],
        }
    }

    pub fn quad_box(self, min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 4] {
        let p = |dx: usize, dy: usize, dz: usize| {
            [
                if dx == 0 { min[0] } else { max[0] },
                if dy == 0 { min[1] } else { max[1] },
                if dz == 0 { min[2] } else { max[2] },
            ]
        };
        match self {
            Face::PosX => [p(1, 0, 1), p(1, 0, 0), p(1, 1, 0), p(1, 1, 1)],
            Face::NegX => [p(0, 0, 0), p(0, 0, 1), p(0, 1, 1), p(0, 1, 0)],
            Face::PosY => [p(0, 1, 1), p(1, 1, 1), p(1, 1, 0), p(0, 1, 0)],
            Face::NegY => [p(0, 0, 0), p(1, 0, 0), p(1, 0, 1), p(0, 0, 1)],
            Face::PosZ => [p(0, 0, 1), p(1, 0, 1), p(1, 1, 1), p(0, 1, 1)],
            Face::NegZ => [p(1, 0, 0), p(0, 0, 0), p(0, 1, 0), p(1, 1, 0)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::FACE_NEIGHBORS;
    use glam::Vec3;

    #[test]
    fn face_order_matches_the_neighbour_table() {
        for (i, face) in Face::ALL.into_iter().enumerate() {
            assert_eq!(face as usize, i);
            assert_eq!(face.dir(), FACE_NEIGHBORS[i]);
            assert_eq!(face.dir().abs().element_sum(), 1, "{face:?} is a unit axis");
        }
        for pair in Face::ALL.chunks(2) {
            assert_eq!(pair[0].dir(), -pair[1].dir());
        }
    }

    #[test]
    fn ao_tangents_span_the_face_plane() {
        for face in Face::ALL {
            let (n, u, v) = (face.dir(), face.ao_u(), face.ao_v());
            assert_eq!(n.dot(u), 0, "{face:?} ao_u lies in the face plane");
            assert_eq!(n.dot(v), 0, "{face:?} ao_v lies in the face plane");
            assert_eq!(u.dot(v), 0, "{face:?} tangents are distinct axes");
        }
    }

    #[test]
    fn quad_box_winds_counter_clockwise_from_outside() {
        for face in Face::ALL {
            let q = face.quad_box([0.0; 3], [1.0; 3]).map(Vec3::from);
            let n = (q[1] - q[0]).cross(q[2] - q[0]);
            assert!(n.dot(face.dir().as_vec3()) > 0.0, "{face:?} winds CCW");
            let axis = face.dir().abs().as_vec3();
            let side = if face.dir().element_sum() > 0 {
                1.0
            } else {
                0.0
            };
            for c in q {
                assert_eq!(c.dot(axis), side, "{face:?} corner {c} on its plane");
            }
        }
    }

    #[test]
    fn ao_signs_follow_quad_box_corner_order() {
        for face in Face::ALL {
            let corners = face.quad_box([0.0; 3], [1.0; 3]).map(Vec3::from);
            let (u, v) = (face.ao_u().as_vec3(), face.ao_v().as_vec3());
            let sign = |x: f32| if x > 0.5 { 1 } else { -1 };
            let derived = corners.map(|c| (sign(c.dot(u)), sign(c.dot(v))));
            assert_eq!(face.ao_signs(), derived, "{face:?}");
        }
    }

    #[test]
    fn quad_box_spans_arbitrary_extents() {
        let (min, max) = ([0.25, -1.0, 2.0], [0.75, 3.0, 2.5]);
        for face in Face::ALL {
            for c in face.quad_box(min, max) {
                for a in 0..3 {
                    assert!(c[a] == min[a] || c[a] == max[a], "{face:?} axis {a}");
                }
            }
        }
    }
}
