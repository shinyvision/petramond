use super::route::Route;
use super::rows::Row;
use crate::body::TICK_SECONDS;
use crate::strike::relative;
use mod_sdk::*;

const CATCH_GRACE_TICKS: u32 = 60;

pub(super) struct Flight {
    pub owner: PlayerId,
    row: Row,
    home: [f64; 3],
    forward: [f32; 3],
    across: [f32; 3],
    strength: f32,
    speed: f32,
    outward: u32,
    turn_at: u32,
    age: u32,
    drop_at: Option<u32>,
    leg: Leg,
    hits: Vec<EntityRef>,
}

enum Leg {
    Outbound,
    Curve(Route),
    Recall,
    Past([f32; 3]),
}

fn length(v: [f32; 3]) -> f32 {
    v.iter().map(|v| v * v).sum::<f32>().sqrt()
}

impl Flight {
    pub fn new(owner: PlayerId, row: Row, forward: [f32; 3], home: [f64; 3], ticks: u32) -> Self {
        let strength = if row.draw.full_ticks == 1 {
            1.0
        } else {
            (ticks.clamp(1, row.draw.full_ticks) - 1) as f32 / (row.draw.full_ticks - 1) as f32
        };
        let [weak, full] = row.outbound_ticks;
        let outward = (weak as f32 + (full - weak) as f32 * strength).round() as u32;
        let horizontal = forward[0].hypot(forward[2]);
        let across = if horizontal > 1e-6 {
            [forward[2] / horizontal, 0.0, -forward[0] / horizontal]
        } else {
            [1.0, 0.0, 0.0]
        };
        let speed = row.draw.launch_speed(ticks);
        // Leave braking room before the bend instead of snapping to the cornering speed.
        let braking_ticks = (speed / (2.0 * row.turn_acceleration * TICK_SECONDS)).ceil() as u32;
        let turn_at = outward.saturating_sub(braking_ticks).max(1);
        Self {
            owner,
            row,
            home,
            forward,
            across,
            strength,
            speed,
            outward,
            turn_at,
            age: 0,
            drop_at: None,
            leg: Leg::Outbound,
            hits: Vec::new(),
        }
    }

    pub fn outbound_velocity(&self) -> [f32; 3] {
        self.forward.map(|v| v * self.speed)
    }

    pub fn step(&mut self, pos: [f64; 3]) -> Option<[f32; 3]> {
        self.age += 1;
        if self.drop_at.is_some_and(|tick| self.age >= tick) {
            return None;
        }
        if matches!(self.leg, Leg::Outbound) && self.age >= self.turn_at {
            let route = Route::new(
                pos,
                self.home,
                self.forward,
                self.across,
                &self.row,
                self.speed,
                (self.outward - self.age) as f32 * self.speed * TICK_SECONDS,
            );
            self.drop_at = Some(self.age - 1 + route.catch_ticks() + CATCH_GRACE_TICKS);
            self.leg = Leg::Curve(route);
        }
        let vel = match &mut self.leg {
            Leg::Outbound => self.outbound_velocity(),
            Leg::Curve(route) => {
                let vel = route.advance(pos);
                if route.finished() {
                    self.leg = Leg::Past(route.tail_velocity());
                }
                vel
            }
            Leg::Recall => {
                let to = relative(self.home, pos);
                let distance = length(to);
                // Further body hits cannot keep extending an uncaught flight's deadline.
                self.drop_at.get_or_insert_with(|| {
                    self.age - 1
                        + (distance / (self.row.return_speed * TICK_SECONDS)).ceil() as u32
                        + CATCH_GRACE_TICKS
                });
                if distance < self.row.return_speed * TICK_SECONDS {
                    let vel = to.map(|v| v / distance.max(1e-6) * self.row.return_speed);
                    self.leg = Leg::Past(vel);
                    vel
                } else {
                    to.map(|v| v / distance * self.row.return_speed)
                }
            }
            Leg::Past(vel) => *vel,
        };
        Some(vel)
    }

    pub fn reverse(&mut self, vel: [f32; 3]) -> [f32; 3] {
        self.leg = Leg::Recall;
        let speed = length(vel).max(1e-6);
        vel.map(|v| -v / speed * self.row.return_speed)
    }

    pub fn damage(&self) -> [f32; 2] {
        std::array::from_fn(|i| {
            self.row.damage_weak[i]
                + (self.row.damage_full[i] - self.row.damage_weak[i]) * self.strength
        })
    }

    pub fn strike(&mut self, body: EntityRef) -> bool {
        if self.hits.contains(&body) {
            return false;
        }
        self.hits.push(body);
        true
    }
}
