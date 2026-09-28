use crate::GenRng;

use super::frame::Frame;
use super::material::{Dir, Half, Material};
use super::plan::Plan;
use super::rng::Draw;

/// A roof's blocks: `stairs(dir)` is a stair whose low side faces `dir`; `slab` is the ridge
/// and flat-roof material; `hole` is the chance any one roof block is missing.
pub struct RoofStyle<'a> {
    pub stairs: &'a dyn Fn(Dir) -> Material,
    pub slab: Material,
    pub hole: f32,
}

fn put(plan: &mut Plan, frame: &Frame, lx: i32, lz: i32, y: i32, material: &Material) {
    let [x, z] = frame.at(lx, lz);
    plan.set([x, y, z], material);
}

/// A gable roof over a `w` × `d` local footprint, ridge along local x, one-block overhang all
/// round, its lowest course at `y0`. `fill` closes the gable ends above the walls.
pub fn gable(
    plan: &mut Plan,
    rng: &mut GenRng,
    frame: &Frame,
    [w, d]: [i32; 2],
    y0: i32,
    style: &RoofStyle,
    mut fill: Option<&mut dyn FnMut(&mut GenRng) -> Material>,
) {
    let ridge = style.slab.half(Half::Bottom);
    let (back, front) = ((style.stairs)(frame.back()), (style.stairs)(frame.fwd()));
    for k in 0.. {
        let (zb, zf, y) = (k - 1, d - k, y0 + k);
        if zb > zf {
            break;
        }
        for lx in -1..=w {
            if zb == zf {
                if !rng.roll(style.hole) {
                    put(plan, frame, lx, zb, y, &ridge);
                }
                continue;
            }
            if !rng.roll(style.hole) {
                put(plan, frame, lx, zb, y, &back);
            }
            if !rng.roll(style.hole) {
                put(plan, frame, lx, zf, y, &front);
            }
        }
        if let Some(fill) = fill.as_mut() {
            for lx in [0, w - 1] {
                for lz in zb + 1..zf {
                    let m = fill(rng);
                    put(plan, frame, lx, lz, y, &m);
                }
            }
        }
        if zb + 1 >= zf {
            break;
        }
    }
}

/// A single slope of slabs falling half a block per row toward local +z, from standing level
/// `high` over the back row (local z = -1 overhang through `d`).
pub fn lean_to(
    plan: &mut Plan,
    rng: &mut GenRng,
    frame: &Frame,
    [w, d]: [i32; 2],
    high: f32,
    style: &RoofStyle,
) {
    for lz in -1..=d {
        let level = high - 0.5 * lz.max(0) as f32;
        for lx in 0..w {
            if rng.roll(style.hole) {
                continue;
            }
            if level.fract() == 0.0 {
                let slab = style.slab.half(Half::Top);
                put(plan, frame, lx, lz, level as i32 - 1, &slab);
            } else {
                let slab = style.slab.half(Half::Bottom);
                put(plan, frame, lx, lz, level.floor() as i32, &slab);
            }
        }
    }
}

/// An A-frame of stairs straight on the ground: `w` wide across local x, `d` long toward local
/// +z, open at the front, closed at the back with `back`.
pub fn a_frame(
    plan: &mut Plan,
    rng: &mut GenRng,
    frame: &Frame,
    [w, d]: [i32; 2],
    y0: i32,
    style: &RoofStyle,
    back: Option<&Material>,
) {
    let ridge = style.slab.half(Half::Bottom);
    let (left, right) = ((style.stairs)(frame.left()), (style.stairs)(frame.right()));
    for k in 0..(w + 1) / 2 {
        let (l, r, y) = (k, w - 1 - k, y0 + k);
        for lz in 0..d {
            if l == r {
                if !rng.roll(style.hole) {
                    put(plan, frame, l, lz, y, &ridge);
                }
                continue;
            }
            if !rng.roll(style.hole) {
                put(plan, frame, l, lz, y, &left);
            }
            if !rng.roll(style.hole) {
                put(plan, frame, r, lz, y, &right);
            }
        }
        if let Some(back) = back {
            for lx in l + 1..r {
                put(plan, frame, lx, 0, y, back);
            }
        }
    }
}
