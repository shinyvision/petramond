use super::face::Face;
#[inline]
pub fn cell_uv(face: Face, p: [f32; 3]) -> [f32; 2] {
    match face {
        Face::PosX => [1.0 - p[2], 1.0 - p[1]],
        Face::NegX => [p[2], 1.0 - p[1]],
        Face::PosY => [p[0], p[2]],
        Face::NegY => [p[0], 1.0 - p[2]],
        Face::PosZ => [p[0], 1.0 - p[1]],
        Face::NegZ => [1.0 - p[0], 1.0 - p[1]],
    }
}

pub fn face_fraction(face: Face, min: [f32; 3], max: [f32; 3], uv: (f32, f32)) -> (f32, f32) {
    FaceUvSpan::of(face, min, max).fraction(uv)
}

#[derive(Clone, Copy)]
pub(super) struct FaceUvSpan {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

impl FaceUvSpan {
    pub(super) fn of(face: Face, min: [f32; 3], max: [f32; 3]) -> Self {
        let (mut u0, mut v0) = (f32::INFINITY, f32::INFINITY);
        let (mut u1, mut v1) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for c in face.quad_box(min, max) {
            let [cu, cv] = cell_uv(face, c);
            u0 = u0.min(cu);
            v0 = v0.min(cv);
            u1 = u1.max(cu);
            v1 = v1.max(cv);
        }
        Self { u0, v0, u1, v1 }
    }

    #[inline]
    pub(super) fn fraction(&self, (u, v): (f32, f32)) -> (f32, f32) {
        let frac = |x: f32, lo: f32, hi: f32| {
            if hi - lo <= 1e-6 {
                0.0
            } else {
                ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
            }
        };
        (frac(u, self.u0, self.u1), frac(v, self.v0, self.v1))
    }
}

pub(super) struct PlaneLight {
    pub(super) ao: [u32; 4],
    pub(super) sky: [u32; 4],
    pub(super) block: [petramond_world::light::BlockLight6; 4],
}

impl PlaneLight {
    pub(super) fn sample(&self, u: f32, v: f32) -> (u32, u32, petramond_world::light::BlockLight6) {
        let w = [(1.0 - u) * v, u * v, u * (1.0 - v), (1.0 - u) * (1.0 - v)];
        let blend = |c: [u32; 4]| -> u32 {
            let f =
                c[0] as f32 * w[0] + c[1] as f32 * w[1] + c[2] as f32 * w[2] + c[3] as f32 * w[3];
            (f + 0.5) as u32
        };
        let b = &self.block;
        let ch = [
            b[0].channels(),
            b[1].channels(),
            b[2].channels(),
            b[3].channels(),
        ];
        let block = petramond_world::light::BlockLight6::new(
            blend([ch[0][0], ch[1][0], ch[2][0], ch[3][0]]),
            blend([ch[0][1], ch[1][1], ch[2][1], ch[3][1]]),
            blend([ch[0][2], ch[1][2], ch[2][2], ch[3][2]]),
        );
        (blend(self.ao), blend(self.sky), block)
    }
}

#[cfg(test)]
mod tests {
    use super::super::face::FACES;
    use super::*;

    #[test]
    fn cell_uv_matches_full_cube_face_orientation() {
        const CORNER_LOCAL: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for face in FACES {
            let corners = face.quad_box([0.0; 3], [1.0; 3]);
            for (i, p) in corners.into_iter().enumerate() {
                assert_eq!(
                    cell_uv(face, p),
                    CORNER_LOCAL[i],
                    "{face:?} corner {i} must match the shader's corner_local"
                );
            }
        }
    }

    /// The UV turn must exactly undo what turning the shape did to a face's
    /// cell-local UV: sampling a turned box at the turned point has to land on
    /// the same texel as sampling the authored box at the authored point, or a
    /// tile authored once cannot serve all four facings.
    ///
    /// The sides come out right for free; `+Y`/`-Y` are the two that need the
    /// correction, in OPPOSITE directions, which is exactly the pair a
    /// hand-derived sign gets backwards.
    #[test]
    fn the_uv_turn_undoes_the_shape_turn_on_every_face() {
        use petramond_world::block::{face_uv_turns, ShapeFace, FACE_BEFORE_TURN};

        let face_before_turns =
            |i: usize, turns: u8| (0..turns).fold(i, |f, _| FACE_BEFORE_TURN[f]);
        let authored = [3.0 / 16.0, 5.0 / 16.0, 6.0 / 16.0];
        for turns in 0..4u8 {
            let mut p = authored;
            for _ in 0..turns {
                p = [1.0 - p[2], p[1], p[0]];
            }
            for (i, face) in FACES.into_iter().enumerate() {
                let want = cell_uv(FACES[face_before_turns(i, turns)], authored);
                let [u, v] = cell_uv(face, p);
                let got = ShapeFace::turn_uv(face_uv_turns(i, turns), u, v);
                assert!(
                    (got.0 - want[0]).abs() < 1e-5 && (got.1 - want[1]).abs() < 1e-5,
                    "turn {turns} face {i}: sampled {got:?}, authored {want:?}"
                );
            }
        }
    }
}
