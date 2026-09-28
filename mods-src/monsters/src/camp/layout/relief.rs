//! Plateaus raised from the rock under the wall line, and the bridges between them.

use std::f32::consts::TAU;

use mod_sdk::build::{span, Draw, Noise2};

use super::*;
use crate::camp::{Camp, Res};

impl Camp<'_> {
    pub(super) fn lay_plateaus(&mut self) {
        let count = *self
            .rng
            .weighted(&[(0usize, 15.0), (1, 20.0), (2, 40.0), (3, 25.0)]);
        let mut angles: Vec<f32> = Vec::new();
        for _ in 0..200 {
            if angles.len() >= count {
                break;
            }
            let a = self.rng.range(0.0, TAU);
            if angles.iter().all(|&b| angle_gap(a, b) > 1.3) {
                angles.push(a);
            }
        }
        for th in angles {
            let rp = self.rng.range(3.8, 6.5) * (self.radius / 19.0).clamp(0.85, 1.25);
            let rr = self.radius_at(th) - self.rng.range(0.1, 0.5) * rp;
            let center = [
                self.center[0] as f32 + rr * th.cos(),
                self.center[1] as f32 + rr * th.sin(),
            ];
            // Per harmonic k = 1, 2, 3: its amplitude and its phase's cosine and sine.
            let shape: [(f32, f32, f32); 3] = std::array::from_fn(|_| {
                let (amp, p) = (self.rng.range(0.0, 0.2), self.rng.range(0.0, TAU));
                (amp, p.cos(), p.sin())
            });
            let rim = Noise2(self.rng.next_u64() as u32);
            let box_cells = |r: f32| {
                let (x0, x1) = (
                    (center[0] - r).floor() as i32,
                    (center[0] + r).ceil() as i32,
                );
                let (z0, z1) = (
                    (center[1] - r).floor() as i32,
                    (center[1] + r).ceil() as i32,
                );
                (z0..=z1).flat_map(move |z| (x0..=x1).map(move |x| [x, z]))
            };
            let dist = |c: [i32; 2]| {
                let (dx, dz) = (c[0] as f32 - center[0], c[1] as f32 - center[1]);
                (dx * dx + dz * dz).sqrt()
            };
            let under: Vec<i32> = box_cells(rp)
                .filter(|&c| self.ground.contains(c) && dist(c) <= rp)
                .map(|c| self.g(c))
                .collect();
            if under.is_empty() {
                continue;
            }
            let lift = self.rng.int(4, 7);
            let top = (under.iter().sum::<i32>() as f32 / under.len() as f32).round() as i32 + lift;
            let id = self.plateaus.len() as u8 + 1;
            let mut cells = Vec::new();
            for c in box_cells(rp * 1.4).collect::<Vec<_>>() {
                if !self.ground.contains(c) {
                    continue;
                }
                let d = dist(c);
                let (dx, dz) = (c[0] as f32 - center[0], c[1] as f32 - center[1]);
                let (cos, sin) = if d > 0.0 {
                    (dx / d, dz / d)
                } else {
                    (1.0, 0.0)
                };
                let (mut ck, mut sk) = (cos, sin);
                let mut rad = rp;
                for &(amp, cos_p, sin_p) in &shape {
                    rad += rp * amp * (sk * cos_p + ck * sin_p);
                    (ck, sk) = (ck * cos - sk * sin, sk * cos + ck * sin);
                }
                if d > rad {
                    continue;
                }
                let y = if d > rad * 0.78 && rim.at(c[0] as f32 / 2.5, c[1] as f32 / 2.5) > 0.0 {
                    top - 1
                } else {
                    top
                };
                if y > self.g(c) {
                    self.ground.set(c, y);
                    self.plateau_at.set(c, id);
                    cells.push(c);
                }
            }
            self.plateaus.push(Plateau {
                id,
                center,
                top,
                cells,
            });
        }
    }

    // Bridges join plateau tops along a minimum spanning tree.
    pub(super) fn lay_bridges(&mut self) {
        if self.plateaus.len() < 2 {
            return;
        }
        let mut edges = Vec::new();
        for a in 0..self.plateaus.len() {
            for b in a + 1..self.plateaus.len() {
                let (p, q) = (self.plateaus[a].center, self.plateaus[b].center);
                edges.push((a, b, (p[0] - q[0]).hypot(p[1] - q[1])));
            }
        }
        edges.sort_by(|x, y| x.2.total_cmp(&y.2));
        let mut root: Vec<usize> = (0..self.plateaus.len()).collect();
        fn find(root: &mut Vec<usize>, i: usize) -> usize {
            if root[i] != i {
                let r = find(root, root[i]);
                root[i] = r;
            }
            root[i]
        }
        for (a, b, _) in edges {
            let (ra, rb) = (find(&mut root, a), find(&mut root, b));
            if ra == rb {
                continue;
            }
            root[ra] = rb;
            if let Some(bridge) = self.plan_bridge(a, b) {
                self.bridges.push(bridge);
            }
        }
    }

    pub(super) fn bridge_end(&self, from: usize, toward: [f32; 2]) -> Option<[i32; 2]> {
        self.plateaus[from]
            .cells
            .iter()
            .copied()
            .filter(|&c| self.is_plateau_top(c) && self.inside(c) && self.depth(c) >= 3)
            .min_by(|&p, &q| {
                let d = |c: [i32; 2]| (c[0] as f32 - toward[0]).hypot(c[1] as f32 - toward[1]);
                d(p).total_cmp(&d(q)).then(p.cmp(&q))
            })
    }

    pub(super) fn plan_bridge(&mut self, a: usize, b: usize) -> Option<Bridge> {
        let end_a = self.bridge_end(a, self.plateaus[b].center)?;
        let end_b = self.bridge_end(b, self.plateaus[a].center)?;
        let len = ((end_a[0] - end_b[0]) as f32).hypot((end_a[1] - end_b[1]) as f32);
        if len < 4.0 {
            return None;
        }
        let spacing = self.rng.int(4, 6);
        let deck = span(
            end_a,
            (self.g(end_a) + 1) as f32,
            end_b,
            (self.g(end_b) + 1) as f32,
            spacing,
        );
        for cell in &deck.cells {
            self.reserve(cell.pos, Res::Bridge);
        }
        for &(pos, _) in &deck.supports {
            self.resv.set(pos, Res::Support);
        }
        let rails =
            *self
                .rng
                .weighted(&[(Rails::Both, 5.0), (Rails::One, 3.0), (Rails::Neither, 2.0)]);
        Some(Bridge { span: deck, rails })
    }
}
