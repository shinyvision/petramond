use petramond_world::block_state::LogAxis;

pub use petramond_math::face::Face;
pub use petramond_world::shade::FaceShading;

pub fn log_side_cell_uv(face: Face, axis: LogAxis, local: [f32; 3]) -> Option<[f32; 2]> {
    let axis_idx = match axis {
        LogAxis::X => 0,
        LogAxis::Y => return None,
        LogAxis::Z => 2,
    };
    let normal_idx = match face {
        Face::PosX | Face::NegX => 0,
        Face::PosY | Face::NegY => 1,
        Face::PosZ | Face::NegZ => 2,
    };
    if normal_idx == axis_idx {
        return None;
    }
    let cross_idx = 3 - axis_idx - normal_idx;
    Some([local[cross_idx], 1.0 - local[axis_idx]])
}

pub(super) fn cross_quads(x: f32, y: f32, z: f32, inset: f32) -> [[[f32; 3]; 4]; 2] {
    let lo = inset;
    let hi = 1.0 - inset;
    [
        [
            [x + lo, y, z + lo],
            [x + hi, y, z + hi],
            [x + hi, y + 1.0, z + hi],
            [x + lo, y + 1.0, z + lo],
        ],
        [
            [x + lo, y, z + hi],
            [x + hi, y, z + lo],
            [x + hi, y + 1.0, z + lo],
            [x + lo, y + 1.0, z + hi],
        ],
    ]
}

pub(super) fn crop_quads(x: f32, y: f32, z: f32, inset: f32, drop: f32) -> [[[f32; 3]; 4]; 4] {
    let a = inset;
    let b = 1.0 - a;
    let y0 = y - drop;
    let y1 = y0 + 1.0;
    [
        [
            [x + a, y0, z],
            [x + a, y0, z + 1.0],
            [x + a, y1, z + 1.0],
            [x + a, y1, z],
        ],
        [
            [x + b, y0, z],
            [x + b, y0, z + 1.0],
            [x + b, y1, z + 1.0],
            [x + b, y1, z],
        ],
        [
            [x, y0, z + a],
            [x + 1.0, y0, z + a],
            [x + 1.0, y1, z + a],
            [x, y1, z + a],
        ],
        [
            [x, y0, z + b],
            [x + 1.0, y0, z + b],
            [x + 1.0, y1, z + b],
            [x, y1, z + b],
        ],
    ]
}

pub(super) const FACES: [Face; 6] = Face::ALL;

#[cfg(test)]
pub(super) fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u32 {
    quad_ao(false, side1, side2, corner)
}

#[inline]
pub(super) fn quad_ao(q_int: bool, side1: bool, side2: bool, corner: bool) -> u32 {
    if (side1 && side2) || (q_int && corner) {
        0
    } else {
        AO_OPEN.saturating_sub(q_int as u32 + side1 as u32 + side2 as u32 + corner as u32)
    }
}

pub(super) const AO_OPEN: u32 = 3;

#[inline]
pub(super) fn should_flip(ao: [u32; 4]) -> bool {
    ao[0] + ao[2] > ao[1] + ao[3]
}

pub(super) fn quad_for(face: Face, x: f32, y: f32, z: f32) -> [[f32; 3]; 4] {
    face.quad_box([x, y, z], [x + 1.0, y + 1.0, z + 1.0])
}
