use super::rows::Row;
use crate::body::TICK_SECONDS;
use crate::strike::{offset_by, relative};

pub(super) struct Route {
    points: Vec<[f64; 3]>,
    speeds: Vec<f32>,
    times: Vec<f32>,
    cursor: usize,
    elapsed: f32,
    tail: [f32; 3],
}

fn length(v: [f32; 3]) -> f32 {
    v.iter().map(|v| v * v).sum::<f32>().sqrt()
}

fn direction(from: [f64; 3], to: [f64; 3]) -> [f32; 3] {
    let delta = relative(to, from);
    let distance = length(delta).max(1e-6);
    delta.map(|v| v / distance)
}

fn angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    length(cross).atan2((0..3).map(|i| a[i] * b[i]).sum())
}

impl Route {
    pub fn new(
        from: [f64; 3],
        home: [f64; 3],
        forward: [f32; 3],
        across: [f32; 3],
        row: &Row,
        launch_speed: f32,
        straight: f32,
    ) -> Self {
        const SAMPLES: usize = 128;
        let turn_start = offset_by(from, forward.map(|v| v * straight));
        let reach = length(relative(turn_start, home)).max(1.0);
        let width = (reach * row.curve).max(row.return_speed * TICK_SECONDS * 1.5);
        let mut turn = vec![[0.0f32; 2]];
        let mut previous = [1.0, 0.0];
        for i in 1..=SAMPLES {
            let u = i as f32 / SAMPLES as f32;
            let heading = std::f32::consts::PI * u * u * (3.0 - 2.0 * u);
            let next = [heading.cos(), heading.sin()];
            let last = turn.last().unwrap();
            turn.push(std::array::from_fn(|axis| {
                last[axis] + (previous[axis] + next[axis]) * 0.5 / SAMPLES as f32
            }));
            previous = next;
        }
        let scale = width / turn.last().unwrap()[1];
        let mut points: Vec<_> = turn
            .iter()
            .map(|offset| {
                offset_by(
                    turn_start,
                    std::array::from_fn(|axis| {
                        scale * (forward[axis] * offset[0] + across[axis] * offset[1])
                    }),
                )
            })
            .collect();
        if straight > 0.0 {
            points.insert(0, from);
        }
        let end = *points.last().unwrap();
        let controls = [
            end,
            offset_by(end, forward.map(|v| -v * reach * 0.2)),
            offset_by(end, forward.map(|v| -v * reach * 0.4)),
            offset_by(
                home,
                std::array::from_fn(|axis| {
                    forward[axis] * reach * 0.4 + across[axis] * width * 0.2
                }),
            ),
            offset_by(
                home,
                std::array::from_fn(|axis| {
                    forward[axis] * reach * 0.2 + across[axis] * width * 0.1
                }),
            ),
            home,
        ];
        // Zero curvature at both ends joins the ramped turn and the final miss tangent.
        for i in 1..=SAMPLES {
            let t = i as f64 / SAMPLES as f64;
            let u = 1.0 - t;
            let weights = [
                u.powi(5),
                5.0 * u.powi(4) * t,
                10.0 * u.powi(3) * t * t,
                10.0 * u * u * t.powi(3),
                5.0 * u * t.powi(4),
                t.powi(5),
            ];
            let offset: [f64; 3] = std::array::from_fn(|axis| {
                controls
                    .iter()
                    .zip(weights)
                    .map(|(p, w)| (p[axis] - home[axis]) * w)
                    .sum::<f64>()
            });
            points.push(std::array::from_fn(|axis| home[axis] + offset[axis]));
        }
        Self::timed(
            points,
            launch_speed,
            row.return_speed,
            row.turn_acceleration,
        )
    }

    fn timed(points: Vec<[f64; 3]>, launch: f32, maximum: f32, acceleration: f32) -> Self {
        let distances: Vec<_> = points
            .windows(2)
            .map(|p| length(relative(p[1], p[0])))
            .collect();
        let directions: Vec<_> = points.windows(2).map(|p| direction(p[0], p[1])).collect();
        let mut speeds = vec![maximum; points.len()];
        for i in 1..points.len() - 1 {
            let curvature = angle(directions[i - 1], directions[i])
                / ((distances[i - 1] + distances[i]) * 0.5).max(1e-6);
            if curvature > 1e-6 {
                speeds[i] = maximum.min((acceleration / curvature).sqrt());
            }
        }
        speeds[0] = speeds[0].min(launch);
        // Look ahead so braking starts before the tightest bend rather than at its apex.
        for i in (0..distances.len()).rev() {
            speeds[i] =
                speeds[i].min((speeds[i + 1].powi(2) + 2.0 * acceleration * distances[i]).sqrt());
        }
        for i in 0..distances.len() {
            speeds[i + 1] =
                speeds[i + 1].min((speeds[i].powi(2) + 2.0 * acceleration * distances[i]).sqrt());
        }
        let mut times = vec![0.0];
        for (i, distance) in distances.iter().enumerate() {
            times.push(times[i] + 2.0 * distance / (speeds[i] + speeds[i + 1]).max(1e-6));
        }
        let tail = directions
            .last()
            .unwrap()
            .map(|v| v * speeds.last().unwrap());
        Self {
            points,
            speeds,
            times,
            cursor: 0,
            elapsed: 0.0,
            tail,
        }
    }

    pub fn advance(&mut self, pos: [f64; 3]) -> [f32; 3] {
        self.elapsed += TICK_SECONDS;
        while self.cursor + 1 < self.times.len() && self.elapsed >= self.times[self.cursor + 1] {
            self.cursor += 1;
        }
        let at = if self.finished() {
            offset_by(
                *self.points.last().unwrap(),
                self.tail
                    .map(|v| v * (self.elapsed - self.times.last().unwrap())),
            )
        } else {
            let i = self.cursor;
            let duration = self.times[i + 1] - self.times[i];
            let dt = self.elapsed - self.times[i];
            let acceleration = (self.speeds[i + 1] - self.speeds[i]) / duration;
            let distance = (self.speeds[i] * dt + 0.5 * acceleration * dt * dt)
                / ((self.speeds[i] + self.speeds[i + 1]) * 0.5 * duration);
            offset_by(
                self.points[i],
                relative(self.points[i + 1], self.points[i]).map(|v| v * distance.clamp(0.0, 1.0)),
            )
        };
        relative(at, pos).map(|v| v / TICK_SECONDS)
    }

    pub fn finished(&self) -> bool {
        self.cursor + 1 == self.times.len()
    }

    pub fn catch_ticks(&self) -> u32 {
        (self.times.last().unwrap() / TICK_SECONDS).ceil() as u32
    }

    pub fn tail_velocity(&self) -> [f32; 3] {
        self.tail
    }
}
