//! What stands on the wall line: gates, watch towers and fortress sections.

use std::f32::consts::TAU;

use mod_sdk::build::{Dir, Draw};

use super::*;
use crate::camp::Res;

impl Planner<'_> {
    pub(super) fn gate_site_ok(
        &self,
        i: usize,
        strict: bool,
        count: usize,
        chosen: &[usize],
    ) -> bool {
        let c = self.outline.ring[i];
        let h0 = self.g(c);
        for k in -3i64..=3 {
            let cc = self.outline.ring[self.outline.wrap(i as i64 + k)];
            if strict && (self.g(cc) - h0).abs() > 1 {
                return false;
            }
            if self.layout.near_plateau(cc, if strict { 2 } else { 0 }) {
                return false;
            }
            for dz in -2..=2 {
                for dx in -2..=2 {
                    let r = self.layout.resv.get([cc[0] + dx, cc[1] + dz]);
                    if r == Res::Bridge || r == Res::Support {
                        return false;
                    }
                }
            }
        }
        chosen.iter().all(|&g| {
            self.outline.ring_dist(g, i) as f32
                >= self.outline.ring_len() as f32 / (count + 1) as f32
        })
    }

    pub(super) fn lay_gates(&mut self) {
        let count = *self.rng.weighted(&[(1usize, 35.0), (2, 40.0), (3, 25.0)]);
        let len = self.outline.ring_len();
        let mut chosen: Vec<usize> = Vec::new();
        let find_near = |camp: &Planner<'_>, chosen: &[usize], target: usize, span: usize| {
            for strict in [true, false] {
                for d in 0..=span {
                    for s in [1i64, -1] {
                        let i = camp.outline.wrap(target as i64 + s * d as i64);
                        if camp.gate_site_ok(i, strict, count, chosen) {
                            return Some(i);
                        }
                    }
                }
            }
            None
        };
        let start = self.rng.int(0, len as i32 - 1) as usize;
        if let Some(first) = find_near(self, &chosen, start, len / 2) {
            chosen.push(first);
            for k in 1..count {
                let jitter = self.rng.range(-0.12, 0.12) * (len / count) as f32;
                let target = self
                    .outline
                    .wrap(first as i64 + ((k * len) as f32 / count as f32 + jitter).round() as i64);
                if let Some(j) = find_near(self, &chosen, target, len / (3 * count)) {
                    chosen.push(j);
                }
            }
        }
        for i in chosen {
            let w = *self.rng.weighted(&[(3usize, 6.0), (2, 2.0), (4, 2.0)]);
            let from = self.outline.wrap(i as i64 - ((w as i64 - 1) >> 1));
            let idx: Vec<usize> = (0..w)
                .map(|k| self.outline.wrap((from + k) as i64))
                .collect();
            let c = self.outline.ring[i];
            let (ox, oz) = (
                (c[0] - self.outline.center[0]) as f32,
                (c[1] - self.outline.center[1]) as f32,
            );
            let l = ox.hypot(oz).max(1e-3);
            self.layout.gates.push(Gate {
                i,
                to: idx[w - 1],
                idx,
                from,
                out: [ox / l, oz / l],
            });
        }
        for gi in 0..self.layout.gates.len() {
            for k in 0..self.layout.gates[gi].idx.len() {
                let j = self.layout.gates[gi].idx[k];
                self.layout.in_gate[j] = true;
                self.layout.resv.set(self.outline.ring[j], Res::Gate);
            }
            let (c, out) = (
                self.outline.ring[self.layout.gates[gi].i],
                self.layout.gates[gi].out,
            );
            for s in 1..=4 {
                for side in [-1.0f32, 0.0, 1.0] {
                    for sign in [-1.0f32, 1.0] {
                        let p = [
                            (c[0] as f32 + sign * out[0] * s as f32 - out[1] * side).round() as i32,
                            (c[1] as f32 + sign * out[1] * s as f32 + out[0] * side).round() as i32,
                        ];
                        self.layout.reserve(p, Res::Path);
                    }
                }
            }
        }
    }

    pub(super) fn try_tower(&mut self, i: usize, flank: bool) -> bool {
        let s = if self.rng.roll(0.7) { 3 } else { 4 };
        let c = self.outline.ring[i];
        let min = [c[0] - 1, c[1] - 1];
        for z in min[1] - 1..=min[1] + s {
            for x in min[0] - 1..=min[0] + s {
                let p = [x, z];
                if !self.ground.contains(p) {
                    return false;
                }
                let r = self.layout.resv.get(p);
                if matches!(r, Res::Tower | Res::Bridge | Res::Support | Res::Centre) {
                    return false;
                }
                let foot = x >= min[0] && x < min[0] + s && z >= min[1] && z < min[1] + s;
                let ri = self.outline.ring_at.get(p);
                if foot && ri >= 0 && self.layout.in_gate[ri as usize] {
                    return false;
                }
            }
        }
        if !flank
            && self.layout.gates.iter().any(|g| {
                (self.outline.ring_dist(g.i, i) as f32) < g.idx.len() as f32 / 2.0 + s as f32 + 2.0
            })
        {
            return false;
        }
        let towers_ring: Vec<usize> = self
            .layout
            .towers
            .iter()
            .map(|t| {
                self.outline
                    .ring_at
                    .get([t.min[0] + 1, t.min[1] + 1])
                    .max(0) as usize
            })
            .collect();
        if towers_ring
            .iter()
            .any(|&t| self.outline.ring_dist(t, i) < 9)
        {
            return false;
        }
        let cx = min[0] as f32 + (s - 1) as f32 / 2.0;
        let cz = min[1] as f32 + (s - 1) as f32 / 2.0;
        let dir = Dir::of(
            self.outline.center[0] as f32 - cx,
            self.outline.center[1] as f32 - cz,
        );
        self.layout.towers.push(Tower { s, min, dir });
        for z in min[1]..min[1] + s {
            for x in min[0]..min[0] + s {
                self.layout.resv.set([x, z], Res::Tower);
            }
        }
        true
    }

    pub(super) fn lay_towers(&mut self) {
        let len = self.outline.ring_len();
        let target = ((TAU * self.outline.radius) / self.rng.range(30.0, 44.0))
            .round()
            .clamp(1.0, 5.0) as usize;
        for gi in 0..self.layout.gates.len() {
            if self.layout.towers.len() < target && self.rng.roll(0.35) {
                let (from, to) = (self.layout.gates[gi].from, self.layout.gates[gi].to);
                let i = if self.rng.roll(0.5) {
                    self.outline.wrap(to as i64 + 2)
                } else {
                    self.outline.wrap(from as i64 - 2)
                };
                self.try_tower(i, true);
            }
        }
        let base = self.rng.int(0, len as i32 - 1) as i64;
        let rem = target.saturating_sub(self.layout.towers.len());
        for k in 0..rem {
            let jitter = self.rng.range(-0.1, 0.1) * (len / rem.max(1)) as f32;
            let t = base + ((k * len) as f32 / rem as f32 + jitter).round() as i64;
            for d in 0..10 {
                let (a, b) = (self.outline.wrap(t + d), self.outline.wrap(t - d));
                if self.try_tower(a, false) || self.try_tower(b, false) {
                    break;
                }
            }
        }
        for _ in 0..200 {
            if !self.layout.towers.is_empty() {
                break;
            }
            let i = self.rng.int(0, len as i32 - 1) as usize;
            self.try_tower(i, true);
        }
    }

    // Thick, walkable wall sections hung off gates and towers.
    pub(super) fn lay_fortress(&mut self) {
        let len = self.outline.ring_len();
        let full = self.rng.roll(0.06);
        let n = if full {
            0
        } else {
            *self
                .rng
                .weighted(&[(0usize, 35.0), (1, 35.0), (2, 20.0), (3, 10.0)])
        };
        let mut anchors: Vec<usize> = self.layout.gates.iter().map(|g| g.i).collect();
        anchors.extend(self.layout.towers.iter().map(|t| {
            self.outline
                .ring_at
                .get([t.min[0] + 1, t.min[1] + 1])
                .max(0) as usize
        }));
        self.rng.shuffle(&mut anchors);
        if full {
            let t = if self.rng.roll(0.6) { 2 } else { 3 };
            self.layout.fort.iter_mut().for_each(|f| *f = t);
            self.layout.fort_arcs.push(FortArc {
                center: 0,
                span: len as i32,
                full: true,
            });
        }
        for k in 0..n {
            let a = anchors
                .get(k)
                .copied()
                .unwrap_or_else(|| self.rng.int(0, len as i32 - 1) as usize);
            let span = self.rng.int(5, 12);
            let t = if self.rng.roll(0.6) { 2 } else { 3 };
            for j in -span..=span {
                let i = self.outline.wrap(a as i64 + j as i64);
                self.layout.fort[i] = self.layout.fort[i].max(t);
            }
            self.layout.fort_arcs.push(FortArc {
                center: a,
                span,
                full: false,
            });
        }
        for i in 0..self.outline.ring.len() {
            self.layout.reserve(self.outline.ring[i], Res::Wall);
        }
        // Fortress cells are ring cells or interior cells at depth 1 or 2.
        for i in 0..self.outline.interior.len() {
            let c = self.outline.interior[i];
            let d = self.outline.depth(c);
            if (1..=2).contains(&d) {
                if let Some(src) = self.outline.field.source(c) {
                    self.layout.inner_by_src.entry(src).or_default().push(c);
                    if d < self.layout.fort[src] {
                        self.layout.reserve(c, Res::Wall);
                    }
                }
            }
        }
    }
}
