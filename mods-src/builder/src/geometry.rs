//! Cell and body arithmetic shared by the golem's planning.

/// The golem row's eye height and a working reach a little short of its row
/// reach, so a stance planned here is one the engine's reach check accepts.
pub const EYE: f64 = 1.3;
pub const PLAN_REACH: f64 = 4.2;

/// The six face neighbours of a cell, and the four beside it.
pub const FACES: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];
pub const SIDES: [[i32; 3]; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];

/// Every face neighbour of every one of `cells`, cell by cell in [`FACES`]
/// order: the cells themselves and repeats included, for the caller to
/// filter as it needs.
pub fn beside(cells: &[[i32; 3]]) -> impl Iterator<Item = [i32; 3]> + '_ {
    cells.iter().flat_map(|c| FACES.map(|f| offset(*c, f)))
}

pub fn cell_of(pos: [f64; 3]) -> [i32; 3] {
    [
        pos[0].floor() as i32,
        (pos[1] + 1.0e-3).floor() as i32,
        pos[2].floor() as i32,
    ]
}

pub fn offset(cell: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [cell[0] + d[0], cell[1] + d[1], cell[2] + d[2]]
}

pub fn feet_of(cell: [i32; 3]) -> [f64; 3] {
    [
        f64::from(cell[0]) + 0.5,
        f64::from(cell[1]),
        f64::from(cell[2]) + 0.5,
    ]
}

/// Distance from the eye of a body standing at `feet` to the nearest point
/// of `cell`.
pub fn reach_to(feet: [f64; 3], cell: [i32; 3]) -> f64 {
    let eye = [feet[0], feet[1] + EYE, feet[2]];
    let mut sum = 0.0;
    for i in 0..3 {
        let lo = f64::from(cell[i]);
        let d = if eye[i] < lo {
            lo - eye[i]
        } else if eye[i] > lo + 1.0 {
            eye[i] - lo - 1.0
        } else {
            0.0
        };
        sum += d * d;
    }
    sum.sqrt()
}

pub fn reaches(feet: [f64; 3], cells: &[[i32; 3]]) -> bool {
    cells.iter().any(|c| reach_to(feet, *c) <= PLAN_REACH)
}

/// The mob yaw facing from `from` toward the centre of `cell` (yaw 0 faces -Z).
pub fn yaw_toward(from: [f64; 3], cell: [i32; 3]) -> f32 {
    let dx = f64::from(cell[0]) + 0.5 - from[0];
    let dz = f64::from(cell[2]) + 0.5 - from[2];
    (-dx).atan2(-dz) as f32
}

pub fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

pub fn encode_cell(cell: [i32; 3]) -> String {
    format!("{} {} {}", cell[0], cell[1], cell[2])
}

pub fn decode_cell(text: &str) -> Option<[i32; 3]> {
    let mut parts = text.split(' ').map(|p| p.parse::<i32>().ok());
    let cell = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(cell)
}

pub fn encode_point(point: [f64; 3]) -> String {
    format!("{:.3} {:.3} {:.3}", point[0], point[1], point[2])
}

pub fn decode_point(text: &str) -> Option<[f64; 3]> {
    let mut parts = text.split(' ').map(|p| p.parse::<f64>().ok());
    let point = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(point)
}

pub fn centre_of(cell: [i32; 3]) -> [f64; 3] {
    [
        f64::from(cell[0]) + 0.5,
        f64::from(cell[1]) + 0.5,
        f64::from(cell[2]) + 0.5,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_measures_to_the_nearest_face_of_a_cell() {
        let feet = [0.5, 0.0, 0.5];
        assert!((reach_to(feet, [0, 1, 0]) - 0.0).abs() < 1e-9);
        assert!((reach_to(feet, [0, 3, 0]) - 1.7).abs() < 1e-9);
        assert!((reach_to(feet, [3, 1, 0]) - 2.5).abs() < 1e-9);
    }
}
