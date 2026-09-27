use std::ops::ControlFlow;
use std::sync::Arc;

use crate::cache::{memo_group, pointee, GenContext};
use crate::rng::FeatureRng;
use petramond_math::detmath;

mod index;
use index::Index;

const CELL: i32 = 64;
const REACH: i32 = 128;

#[derive(Clone, Copy)]
struct Profile {
    salt: u64,
    chance: f32,
    vertical: f64,
    branches: bool,
    floor: f64,
}

const PROFILES: [Profile; 2] = [
    Profile {
        salt: crate::salts::CAVE_WALK_BRANCHING,
        chance: 0.8,
        vertical: 0.8,
        branches: true,
        floor: -0.75,
    },
    Profile {
        salt: crate::salts::CAVE_WALK_STEEP,
        chance: 0.12,
        vertical: 3.0,
        branches: false,
        floor: -1.0,
    },
];

#[derive(Clone, Copy)]
struct Cut {
    center: [f64; 3],
    radius: f64,
    vertical: f64,
    floor: f64,
    extent: [f64; 3],
}

impl Cut {
    fn new(center: [f64; 3], radius: f64, profile: Profile) -> Self {
        let support = 1.0 + 2.0 / (radius * profile.vertical.min(1.0));
        Self {
            center,
            radius,
            vertical: profile.vertical,
            floor: profile.floor,
            extent: [
                radius * support,
                radius * profile.vertical * support,
                radius * support,
            ],
        }
    }

    fn intersects(&self, [lo, hi]: [[i32; 3]; 2]) -> bool {
        (0..3).all(|a| {
            let reach = self.extent[a];
            self.center[a] + reach >= lo[a] as f64 && self.center[a] - reach <= hi[a] as f64
        })
    }

    fn density(&self, p: [f64; 3]) -> f64 {
        if (0..3).any(|a| (p[a] - self.center[a]).abs() > self.extent[a]) {
            return 1.0;
        }
        let dx = (p[0] - self.center[0]) / self.radius;
        let dy = (p[1] - self.center[1]) / (self.radius * self.vertical);
        let dz = (p[2] - self.center[2]) / self.radius;
        ((dx * dx + dy * dy + dz * dz).sqrt() - 1.0).max(self.floor - dy)
            * self.radius
            * self.vertical.min(1.0)
            * 0.25
    }
}

const FIELD_PAD: i32 = 32;

memo_group! {
    pub(super) struct WalkMemos {
        plans: (GenContext, [i32; 2]) => Arc<WalkField> =
            ("cave.walk_plans", 1024, Frontier, pointee),
        fields: (GenContext, [i32; 2]) => Arc<WalkField> =
            ("cave.walk_fields", 512, Frontier, pointee),
    }
}

pub(super) struct WalkField {
    cuts: Vec<Cut>,
    index: Index,
}

impl WalkField {
    pub(super) fn gather(memos: &WalkMemos, context: GenContext, bounds: [[i32; 3]; 2]) -> Self {
        let [lo, hi] = bounds;
        let cell = [lo[0].div_euclid(CELL), lo[2].div_euclid(CELL)];
        let fits = |axis: usize, c: i32| hi[axis] <= c * CELL + CELL - 1 + FIELD_PAD;
        if fits(0, cell[0]) && fits(2, cell[1]) {
            let field = memos.fields.get_or_insert((context, cell), || {
                Arc::new(Self::gather_direct(
                    memos,
                    context,
                    [
                        [
                            cell[0] * CELL - FIELD_PAD,
                            i32::MIN / 2,
                            cell[1] * CELL - FIELD_PAD,
                        ],
                        [
                            cell[0] * CELL + CELL - 1 + FIELD_PAD,
                            i32::MAX / 2,
                            cell[1] * CELL + CELL - 1 + FIELD_PAD,
                        ],
                    ],
                ))
            });
            return field.restrict(bounds);
        }
        Self::gather_direct(memos, context, bounds)
    }

    fn restrict(&self, bounds: [[i32; 3]; 2]) -> Self {
        let mut out = Vec::new();
        let _ = self.index.visit(&self.cuts, bounds, |cut| {
            out.push(cut);
            ControlFlow::Continue(())
        });
        Self::from_cuts(out)
    }

    fn gather_direct(memos: &WalkMemos, context: GenContext, bounds: [[i32; 3]; 2]) -> Self {
        let [lo, hi] = bounds;
        let mut out = Vec::new();
        for z in (lo[2] - REACH).div_euclid(CELL)..=(hi[2] + REACH).div_euclid(CELL) {
            for x in (lo[0] - REACH).div_euclid(CELL)..=(hi[0] + REACH).div_euclid(CELL) {
                let field = memos.plans.get_or_insert((context, [x, z]), || {
                    Arc::new(Self::from_cuts(plan(context.seed(), [x, z])))
                });
                let _ = field.index.visit(&field.cuts, bounds, |cut| {
                    out.push(cut);
                    ControlFlow::Continue(())
                });
            }
        }
        Self::from_cuts(out)
    }

    fn from_cuts(mut cuts: Vec<Cut>) -> Self {
        let index = Index::build(&mut cuts);
        Self { cuts, index }
    }

    pub(super) fn intersects(&self, bounds: [[i32; 3]; 2]) -> bool {
        self.index.intersects(&self.cuts, bounds)
    }

    pub(super) fn at(&self, p: [f64; 3]) -> f64 {
        self.index.at(&self.cuts, p)
    }
}

fn plan(seed: u32, [x, z]: [i32; 2]) -> Vec<Cut> {
    let mut out = Vec::new();
    for profile in PROFILES {
        let mut rng = FeatureRng::positional(seed, profile.salt, x, 0, z);
        if !rng.chance(profile.chance) {
            continue;
        }
        let origin = [
            (x * CELL + rng.next_i32(0, CELL - 1)) as f64,
            rng.next_i32(-32, 80) as f64,
            (z * CELL + rng.next_i32(0, CELL - 1)) as f64,
        ];
        let direction = rng.next_f32() as f64 * std::f64::consts::TAU;
        let length = rng.next_i32(60, 104);
        let width = 1.0 + rng.next_f32() as f64 * 2.4;
        walk(
            &mut out, &mut rng, profile, origin, direction, 0.0, length, width,
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(
    out: &mut Vec<Cut>,
    rng: &mut FeatureRng,
    profile: Profile,
    mut p: [f64; 3],
    mut yaw: f64,
    mut pitch: f64,
    length: i32,
    width: f64,
) {
    let split = rng.next_i32(length / 3, length * 2 / 3);
    let (mut turn, mut rise) = (0.0, 0.0);
    for step in 0..length {
        let t = step as f64 / length as f64;
        let radius = 2.2 + detmath::sin(t * std::f64::consts::PI) * width;
        out.push(Cut::new(p, radius, profile));
        let (yaw_sin, yaw_cos) = detmath::sin_cos(yaw);
        let (pitch_sin, pitch_cos) = detmath::sin_cos(pitch);
        p[0] += yaw_cos * pitch_cos;
        p[2] += yaw_sin * pitch_cos;
        p[1] += pitch_sin;
        turn = turn * 0.75 + (rng.next_f32() - rng.next_f32()) as f64 * 0.35;
        rise = rise * 0.8 + (rng.next_f32() - rng.next_f32()) as f64 * 0.15;
        yaw += turn;
        pitch = pitch * 0.75 + rise;
        if profile.branches && step == split {
            let child = Profile {
                branches: false,
                ..profile
            };
            for side in [-1.0, 1.0] {
                let mut branch = FeatureRng::from_state(rng.next_u64());
                walk(
                    out,
                    &mut branch,
                    child,
                    p,
                    yaw + side * 1.1,
                    pitch / 3.0,
                    length - step,
                    width * 0.5,
                );
            }
            break;
        }
    }
}

#[cfg(test)]
mod tests;
