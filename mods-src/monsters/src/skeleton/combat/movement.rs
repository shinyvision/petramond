use mod_sdk::*;

use super::super::geometry::distance;
use super::{Foe, Turn};

pub(super) const KEEP_DISTANCE: f64 = 6.0;
const WALK_SPEED: f32 = 2.6;
const LOOKAHEAD: f32 = 0.65;
const COMMIT_TICKS: u64 = 12;

#[derive(Default)]
pub(super) struct Retreat {
    direction: Option<[f32; 2]>,
    until: u64,
}

fn candidates(me: [f64; 3], foe: [f64; 3], side: f32) -> Vec<[f32; 2]> {
    let (dx, dz) = ((me[0] - foe[0]) as f32, (me[2] - foe[2]) as f32);
    let len = dx.hypot(dz);
    let away = if len > 1e-4 {
        [dx / len, dz / len]
    } else {
        [1.0, 0.0]
    };
    [0.0f32, 45.0, -45.0, 90.0, -90.0, 135.0, -135.0, 180.0]
        .iter()
        .map(|deg| {
            let (s, c) = (deg.to_radians() * side).sin_cos();
            [away[0] * c - away[1] * s, away[0] * s + away[1] * c]
        })
        .collect()
}

fn separates(me: [f64; 3], foe: [f64; 3], dir: [f32; 2]) -> bool {
    let to = [
        me[0] + f64::from(dir[0] * LOOKAHEAD),
        me[1],
        me[2] + f64::from(dir[1] * LOOKAHEAD),
    ];
    distance(to, foe) > distance(me, foe) + 0.01
}

impl Turn<'_> {
    pub(super) fn retreat(&self, state: &mut Retreat, foe: &Foe) -> Option<[f32; 2]> {
        if distance(self.me.pos, foe.body.feet) > KEEP_DISTANCE || !self.me.on_ground {
            *state = Retreat::default();
            return None;
        }
        let side = if (self.me.id ^ (self.now / 40)) & 1 == 0 {
            1.0
        } else {
            -1.0
        };
        let mut dirs = candidates(self.me.pos, foe.body.feet, side);
        if self.now < state.until {
            if let Some(dir) = state.direction {
                dirs.insert(0, dir);
            }
        }
        dirs.retain(|&dir| separates(self.me.pos, foe.body.feet, dir));
        let safe = mob_walk_probe(
            self.me.id,
            dirs.iter().map(|d| d.map(|v| v * LOOKAHEAD)).collect(),
            1.0,
        );
        let dir = dirs
            .into_iter()
            .zip(safe)
            .find_map(|(dir, safe)| safe.then_some(dir))?;
        if state.direction != Some(dir) || self.now >= state.until {
            state.until = self.now + COMMIT_TICKS;
            state.direction = Some(dir);
        }
        Some(dir.map(|v| v * WALK_SPEED))
    }
}

#[cfg(test)]
mod tests;
