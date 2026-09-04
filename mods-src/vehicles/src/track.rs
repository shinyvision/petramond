//! The geometry of a rail cell: the curve a cart's wheels follow through it.
//!
//! Every form is one PATH from its first exit (`s = 0`) to its second
//! (`s = len`), parametrised by arc length in cell units, in the cell's own
//! frame (`x`, `z` in `0..1` across the cell, `h` the height above its floor).
//! A straight is a centre line, a slope the same line lifted through one
//! cell, a curve a quarter circle about the corner between its exits — the
//! arc that meets both neighbouring straights tangentially, so a cart rounds
//! the corner instead of kinking through it.

use crate::rail::{Axis, Dir, Exit, Form};

/// Where the wheels sit above the cell floor on a level rail: the rail's
/// authored 1-texel height.
pub const RAIL_TOP: f32 = 1.0 / 16.0;

const HALF: f32 = 0.5;
const QUARTER_TURN: f32 = std::f32::consts::FRAC_PI_2;

#[derive(Copy, Clone, Debug, PartialEq)]
enum Shape {
    /// From `a` to `b` in the horizontal plane, rising `rise` over the run.
    Line { a: [f32; 2], b: [f32; 2], rise: f32 },
    /// A quarter circle of radius `HALF` about `center`, from angle `from`
    /// sweeping `sweep` (signed) radians. Angles are in the `(x, z)` plane.
    Arc {
        center: [f32; 2],
        from: f32,
        sweep: f32,
    },
}

/// A rail form's path.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Path {
    shape: Shape,
    exits: [Exit; 2],
}

/// Midpoint of the cell edge `d` leads out through.
fn edge_mid(d: Dir) -> [f32; 2] {
    match d {
        Dir::N => [HALF, 0.0],
        Dir::E => [1.0, HALF],
        Dir::S => [HALF, 1.0],
        Dir::W => [0.0, HALF],
    }
}

/// The corner two adjacent exits share.
fn corner(a: Dir, b: Dir) -> [f32; 2] {
    let x = if a == Dir::E || b == Dir::E { 1.0 } else { 0.0 };
    let z = if a == Dir::S || b == Dir::S { 1.0 } else { 0.0 };
    [x, z]
}

fn angle_of(center: [f32; 2], p: [f32; 2]) -> f32 {
    (p[1] - center[1]).atan2(p[0] - center[0])
}

fn wrap(a: f32) -> f32 {
    let t = a.rem_euclid(std::f32::consts::TAU);
    if t > std::f32::consts::PI {
        t - std::f32::consts::TAU
    } else {
        t
    }
}

impl Path {
    pub fn of(form: Form) -> Path {
        let exits = form.exits();
        let shape = match form {
            Form::Straight(Axis::NS) => Shape::Line {
                a: edge_mid(Dir::N),
                b: edge_mid(Dir::S),
                rise: 0.0,
            },
            Form::Straight(Axis::EW) => Shape::Line {
                a: edge_mid(Dir::W),
                b: edge_mid(Dir::E),
                rise: 0.0,
            },
            Form::Slope(d) => Shape::Line {
                a: edge_mid(d.opposite()),
                b: edge_mid(d),
                rise: 1.0,
            },
            Form::Curve(c) => {
                let [a, b] = c.exits();
                let center = corner(a, b);
                let from = angle_of(center, edge_mid(a));
                let sweep = wrap(angle_of(center, edge_mid(b)) - from);
                debug_assert!((sweep.abs() - QUARTER_TURN).abs() < 1e-4);
                Shape::Arc {
                    center,
                    from,
                    sweep,
                }
            }
        };
        Path { shape, exits }
    }

    /// The exit at `s = 0` and the exit at `s = len`.
    pub fn exits(&self) -> [Exit; 2] {
        self.exits
    }

    /// Arc length of the whole path in cell units.
    pub fn len(&self) -> f32 {
        match self.shape {
            Shape::Line { rise, .. } => (1.0 + rise * rise).sqrt(),
            Shape::Arc { sweep, .. } => sweep.abs() * HALF,
        }
    }

    /// Whether `exit` is the `s = 0` end (`true`) or the `s = len` end
    /// (`false`) — `None` if the path has no such exit.
    pub fn starts_at(&self, exit: Exit) -> Option<bool> {
        if self.exits[0] == exit {
            Some(true)
        } else if self.exits[1] == exit {
            Some(false)
        } else {
            None
        }
    }

    /// Cell-local `[x, h, z]` at arc length `s`.
    pub fn point(&self, s: f32) -> [f32; 3] {
        let t = (s / self.len()).clamp(0.0, 1.0);
        match self.shape {
            Shape::Line { a, b, rise } => {
                [a[0] + (b[0] - a[0]) * t, rise * t, a[1] + (b[1] - a[1]) * t]
            }
            Shape::Arc {
                center,
                from,
                sweep,
            } => {
                let ang = from + sweep * t;
                [
                    center[0] + HALF * ang.cos(),
                    0.0,
                    center[1] + HALF * ang.sin(),
                ]
            }
        }
    }

    /// Unit tangent `[x, y, z]` pointing toward increasing `s`.
    pub fn tangent(&self, s: f32) -> [f32; 3] {
        match self.shape {
            Shape::Line { a, b, rise } => {
                let run = [b[0] - a[0], b[1] - a[1]];
                let n = (1.0 + rise * rise).sqrt();
                [run[0] / n, rise / n, run[1] / n]
            }
            Shape::Arc { from, sweep, .. } => {
                let t = (s / self.len()).clamp(0.0, 1.0);
                let ang = from + sweep * t;
                let dir = sweep.signum();
                [-ang.sin() * dir, 0.0, ang.cos() * dir]
            }
        }
    }

    /// The arc length nearest to the cell-local horizontal point `[x, z]`,
    /// clamped onto the path.
    pub fn project(&self, p: [f32; 2]) -> f32 {
        let len = self.len();
        match self.shape {
            Shape::Line { a, b, .. } => {
                let run = [b[0] - a[0], b[1] - a[1]];
                let t = ((p[0] - a[0]) * run[0] + (p[1] - a[1]) * run[1])
                    / (run[0] * run[0] + run[1] * run[1]);
                (t.clamp(0.0, 1.0) * len).clamp(0.0, len)
            }
            Shape::Arc {
                center,
                from,
                sweep,
            } => {
                let ang = angle_of(center, p);
                let t = wrap(ang - from) / sweep;
                (t.clamp(0.0, 1.0) * len).clamp(0.0, len)
            }
        }
    }
}

/// The grade a cart feels at `s` moving toward increasing `s`: the tangent's
/// vertical component, `> 0` climbing.
pub fn grade(path: &Path, s: f32) -> f32 {
    path.tangent(s)[1]
}

/// `[x, z]` of a 3-vector's horizontal part.
pub fn xz(v: [f32; 3]) -> [f32; 2] {
    [v[0], v[2]]
}

pub fn dot2(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

/// Mob-convention yaw whose facing `(-sin, -cos)` is the horizontal
/// direction `d` (unnormalised).
pub fn yaw_facing(d: [f32; 2]) -> f32 {
    (-d[0]).atan2(-d[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rail::Corner;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn every_path_runs_edge_midpoint_to_edge_midpoint_and_projects_back_onto_itself() {
        for f in Form::ALL {
            let p = Path::of(f);
            let [a, b] = p.exits();
            let start = p.point(0.0);
            let end = p.point(p.len());
            let ea = edge_mid(a.dir);
            let eb = edge_mid(b.dir);
            assert!(
                close(start, [ea[0], 0.0, ea[1]]),
                "{f:?} starts at its first exit: {start:?}"
            );
            assert!(
                close(end, [eb[0], if b.up { 1.0 } else { 0.0 }, eb[1]]),
                "{f:?} ends at its second exit: {end:?}"
            );
            for i in 0..=8 {
                let s = p.len() * i as f32 / 8.0;
                let q = p.point(s);
                let back = p.project([q[0], q[2]]);
                assert!((back - s).abs() < 1e-3, "{f:?} s={s} projected {back}");
                let t = p.tangent(s);
                let n = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
                assert!((n - 1.0).abs() < 1e-4, "{f:?} unit tangent");
            }
        }
    }

    #[test]
    fn a_curve_leaves_each_exit_along_that_exits_direction() {
        // Tangent continuity with the neighbouring straights: at the N exit
        // of a N-E curve the path runs north-south, at the E exit east-west.
        let p = Path::of(Form::Curve(Corner::NE));
        let t0 = p.tangent(0.0);
        let t1 = p.tangent(p.len());
        assert!(t0[0].abs() < 1e-4 && t0[2].abs() > 0.99, "{t0:?}");
        assert!(t1[2].abs() < 1e-4 && t1[0].abs() > 0.99, "{t1:?}");
        // Leaving through the E exit means moving +X.
        assert!(t1[0] > 0.0);
        assert!((p.len() - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
    }

    #[test]
    fn a_slope_climbs_one_cell_over_its_run_at_a_constant_grade() {
        let p = Path::of(Form::Slope(Dir::N));
        assert!((p.len() - std::f32::consts::SQRT_2).abs() < 1e-4);
        assert!((grade(&p, 0.3) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        let foot = p.point(0.0);
        let head = p.point(p.len());
        assert!(close(foot, [0.5, 0.0, 1.0]), "{foot:?}");
        assert!(close(head, [0.5, 1.0, 0.0]), "{head:?}");
        assert!(!p.exits()[0].up && p.exits()[1].up);
    }

    #[test]
    fn projection_clamps_onto_the_path_from_off_cell_points() {
        let p = Path::of(Form::Straight(Axis::EW));
        assert_eq!(p.project([-3.0, 0.5]), 0.0);
        assert_eq!(p.project([9.0, 0.9]), p.len());
        let c = Path::of(Form::Curve(Corner::SW));
        let inside = c.project([0.2, 0.9]);
        assert!(inside > 0.0 && inside < c.len(), "{inside}");
    }
}
