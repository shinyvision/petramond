//! Aiming a launched item: the pitch that carries a shot of a given speed onto a point, found by
//! flying it the way the engine flies an item entity each tick (gravity, then drag, then the move).

use serde::Deserialize;

pub const TICK_SECONDS: f32 = 0.05;

/// Longest flight the solver follows.
const MAX_TICKS: u32 = 400;

const PITCH_LIMIT_DEG: i32 = 80;

const BISECT_STEPS: u32 = 18;

/// How much of a moving target's travel during the flight the shot leads it by.
const LEAD: f32 = 0.7;

/// How an item row flies (`petramond:projectile`); a row without it flies with the engine's
/// defaults.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Flight {
    #[serde(default = "Flight::default_gravity")]
    pub gravity: f32,
    #[serde(default)]
    pub drag: f32,
}

impl Default for Flight {
    fn default() -> Self {
        Flight {
            gravity: Flight::default_gravity(),
            drag: 0.0,
        }
    }
}

impl Flight {
    fn default_gravity() -> f32 {
        20.0
    }

    fn drag_factor(self) -> f32 {
        (1.0 - self.drag).max(0.0).powf(TICK_SECONDS)
    }

    /// One tick of flight: the engine's order.
    pub fn step(self, pos: &mut [f32; 2], vel: &mut [f32; 2]) {
        vel[1] -= self.gravity * TICK_SECONDS;
        let k = self.drag_factor();
        vel[0] *= k;
        vel[1] *= k;
        pos[0] += vel[0] * TICK_SECONDS;
        pos[1] += vel[1] * TICK_SECONDS;
    }

    /// Height a shot launched at `pitch` has when it has flown `reach` horizontally, and after how
    /// many seconds. `None` when it never gets that far.
    pub fn height_at(self, speed: f32, pitch: f32, reach: f32) -> Option<(f32, f32)> {
        let mut pos = [0.0f32, 0.0];
        let mut vel = [speed * pitch.cos(), speed * pitch.sin()];
        for tick in 1..=MAX_TICKS {
            let before = pos;
            self.step(&mut pos, &mut vel);
            if pos[0] >= reach {
                let t = (reach - before[0]) / (pos[0] - before[0]).max(1.0e-6);
                let height = before[1] + (pos[1] - before[1]) * t;
                return Some((height, (tick as f32 - 1.0 + t) * TICK_SECONDS));
            }
            if vel[0] <= 1.0e-4 {
                return None;
            }
        }
        None
    }

    /// The flattest pitch that puts a shot of `speed` at `rise` above the launch point after
    /// `reach` of horizontal travel, and the flight time. `None` when no pitch reaches.
    pub fn solve_pitch(self, speed: f32, reach: f32, rise: f32) -> Option<(f32, f32)> {
        let height = |deg: f32| self.height_at(speed, deg.to_radians(), reach);
        let mut below = None;
        for deg in -PITCH_LIMIT_DEG..=PITCH_LIMIT_DEG {
            let deg = deg as f32;
            match height(deg) {
                Some((h, _)) if h < rise => below = Some(deg),
                Some(_) => {
                    let lo = below?;
                    return Some(self.bisect(speed, reach, rise, lo, deg));
                }
                None => below = None,
            }
        }
        None
    }

    fn bisect(self, speed: f32, reach: f32, rise: f32, mut lo: f32, mut hi: f32) -> (f32, f32) {
        for _ in 0..BISECT_STEPS {
            let mid = 0.5 * (lo + hi);
            match self.height_at(speed, mid.to_radians(), reach) {
                Some((h, _)) if h >= rise => hi = mid,
                _ => lo = mid,
            }
        }
        let pitch = (0.5 * (lo + hi)).to_radians();
        let seconds = self.height_at(speed, pitch, reach).map_or(0.0, |(_, t)| t);
        (pitch, seconds)
    }
}

/// A launch direction (unit) and the flight time to the aimed point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub dir: [f32; 3],
    pub seconds: f32,
}

/// The direction to launch at `speed` from `from` so the shot arrives at `target`.
pub fn aim_at(flight: Flight, speed: f32, from: [f64; 3], target: [f64; 3]) -> Option<Shot> {
    let (dx, dz) = ((target[0] - from[0]) as f32, (target[2] - from[2]) as f32);
    let reach = dx.hypot(dz);
    if reach < 1.0e-3 {
        return None;
    }
    let rise = (target[1] - from[1]) as f32;
    let (pitch, seconds) = flight.solve_pitch(speed, reach, rise)?;
    let (ux, uz) = (dx / reach, dz / reach);
    let (s, c) = pitch.sin_cos();
    Some(Shot {
        dir: [ux * c, s, uz * c],
        seconds,
    })
}

/// [`aim_at`], leading a target that moves at `vel` along the ground.
pub fn aim_leading(
    flight: Flight,
    speed: f32,
    from: [f64; 3],
    target: [f64; 3],
    vel: [f32; 3],
) -> Option<Shot> {
    let first = aim_at(flight, speed, from, target)?;
    let ahead = f64::from(first.seconds * LEAD);
    let led = [
        target[0] + f64::from(vel[0]) * ahead,
        target[1],
        target[2] + f64::from(vel[2]) * ahead,
    ];
    aim_at(flight, speed, from, led).or(Some(first))
}

/// `dir` turned by up to `spread_deg` in yaw and in pitch; `rolls` pick where in that square.
pub fn scatter(dir: [f32; 3], spread_deg: f32, rolls: [u64; 2]) -> [f32; 3] {
    if spread_deg <= 0.0 {
        return dir;
    }
    let unit = |r: u64| (r >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0;
    let spread = spread_deg.to_radians();
    let yaw = dir[0].atan2(dir[2]) + unit(rolls[0]) * spread;
    let pitch = (dir[1].clamp(-1.0, 1.0).asin() + unit(rolls[1]) * spread).clamp(-1.55, 1.55);
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    [sy * cp, sp, cy * cp]
}
