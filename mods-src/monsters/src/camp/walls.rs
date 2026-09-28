use mod_sdk::build::{Axis, Dir, Draw, Half, Name, Noise2};

use super::ground::{Rubble, RubbleKind};
use super::{Camp, Res};

#[derive(Clone, Copy, PartialEq, Eq)]
enum WoodStyle {
    Palisade,
    PostPlank,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PatchKind {
    Fence,
    Planks,
    Logs,
}

impl Camp<'_> {
    fn ring_base(&self, i: usize) -> i32 {
        self.g(self.ring[i]) + 1
    }

    fn col_top(&self, i: usize) -> i32 {
        self.ring_base(i) + self.wall_h[i] - 1
    }

    fn tangent(&self, i: usize) -> Dir {
        let (a, b) = (
            self.ring[self.wrap(i as i64 - 1)],
            self.ring[self.wrap(i as i64 + 1)],
        );
        Dir::of((b[0] - a[0]) as f32, (b[1] - a[1]) as f32)
    }

    fn lower_neighbour(&mut self, i: usize) -> Option<Dir> {
        let me = self.col_top(i);
        let c = self.ring[i];
        let lower: Vec<usize> = [self.wrap(i as i64 - 1), self.wrap(i as i64 + 1)]
            .into_iter()
            .filter(|&j| self.fort[j] == 0 && self.col_top(j) < me)
            .collect();
        if lower.is_empty() {
            return None;
        }
        let j = *self.rng.pick(&lower);
        Some(Dir::of(
            (self.ring[j][0] - c[0]) as f32,
            (self.ring[j][1] - c[1]) as f32,
        ))
    }

    /// The primitive wall: uneven, breached, patched and crumbling.
    pub(super) fn build_walls(&mut self) {
        let len = self.ring_len();
        let noise = Noise2(self.rng.next_u64() as u32);
        let base_h = if self.mats.stone {
            self.rng.int(2, 4)
        } else {
            self.rng.int(3, 4)
        };
        let style = *self
            .rng
            .weighted(&[(WoodStyle::Palisade, 5.0), (WoodStyle::PostPlank, 4.0)]);
        let post_every = self.rng.int(3, 4) as usize;
        for i in 0..len {
            self.wall_h[i] = (base_h + (noise.at(i as f32 / 5.0, 3.3) * 1.3).round() as i32).max(1);
        }
        let breaches = ((len as f32 / self.rng.range(20.0, 32.0)).round() as usize).max(1);
        for _ in 0..breaches {
            let c = self.rng.int(0, len as i32 - 1) as usize;
            let (w, low) = (self.rng.int(1, 4), self.rng.int(0, 1));
            if self.fort[c] > 0 || self.in_gate[c] {
                continue;
            }
            for j in 0..w {
                let k = self.wrap(c as i64 + j as i64 - (w >> 1) as i64);
                if self.fort[k] == 0 {
                    let edge = if j == 0 || j == w - 1 { 1 } else { 0 };
                    self.wall_h[k] = self.wall_h[k].min(low + edge);
                }
            }
            self.rubble.push(Rubble {
                at: self.ring[c],
                r: 2.5,
                n: self.rng.int(3, 6),
                kind: RubbleKind::Camp,
            });
        }
        let mut patches: Vec<Option<(PatchKind, Name)>> = vec![None; len];
        if !self.mats.stone {
            let n = (len as f32 / self.rng.range(16.0, 26.0)).round() as usize;
            for _ in 0..n {
                let (c, run) = (self.rng.int(0, len as i32 - 1), self.rng.int(2, 4));
                let kind = *self.rng.weighted(&[
                    (PatchKind::Fence, 1.0),
                    (PatchKind::Planks, 1.0),
                    (PatchKind::Logs, 0.6),
                ]);
                let wood = if self.rng.roll(0.3) {
                    self.mats.other_wood(&mut self.rng)
                } else {
                    self.mats.wood
                };
                for j in 0..run {
                    patches[self.wrap((c + j) as i64)] = Some((kind, wood));
                }
            }
        }
        for (i, patch) in patches.into_iter().enumerate() {
            if self.fort[i] > 0 || self.in_gate[i] {
                continue;
            }
            if self.mats.stone {
                self.stone_column(i);
            } else if let Some((kind, wood)) = patch {
                self.patch_column(i, kind, wood);
            } else {
                self.wood_column(i, style, i % post_every == 0);
            }
        }
        self.buttresses();
    }

    fn stone_column(&mut self, i: usize) {
        let c = self.ring[i];
        let (base, h) = (self.ring_base(i), self.wall_h[i]);
        for k in 0..h {
            let y = base + k;
            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
            if k == h - 1 {
                let r = self.rng.unit();
                if r < 0.2 {
                    let m = self.mats.slab(s, Half::Bottom);
                    self.put_at(c, y, &m);
                    continue;
                }
                if r < 0.38 {
                    let facing = self.lower_neighbour(i).unwrap_or_else(|| self.tangent(i));
                    let m = self.mats.stairs(s, facing, Half::Bottom);
                    self.put_at(c, y, &m);
                    continue;
                }
            } else if k > 0 {
                let r = self.rng.unit();
                if r < 0.035 {
                    continue;
                }
                if r < 0.06 {
                    let d = *self.rng.pick(&Dir::ALL);
                    let m = self.mats.stairs(s, d, Half::Top);
                    self.put_at(c, y, &m);
                    continue;
                }
            }
            let m = self.mats.block(s);
            self.put_at(c, y, &m);
        }
    }

    fn patch_column(&mut self, i: usize, kind: PatchKind, wood: Name) {
        let c = self.ring[i];
        let (base, h) = (self.ring_base(i), self.wall_h[i]);
        match kind {
            PatchKind::Fence => {
                for k in 0..self.rng.int(1, 3) {
                    let m = self.mats.fence(wood);
                    self.put_at(c, base + k, &m);
                }
            }
            PatchKind::Planks => {
                let ph = (h - 1).max(1);
                for k in 0..ph {
                    let m = self.mats.block(wood);
                    self.put_at(c, base + k, &m);
                }
                if self.rng.roll(0.5) {
                    let m = self.mats.slab(wood, Half::Bottom);
                    self.put_at(c, base + ph, &m);
                }
            }
            PatchKind::Logs => {
                for k in 0..h {
                    let m = self.mats.log(wood, Axis::Y);
                    self.put_at(c, base + k, &m);
                }
            }
        }
    }

    fn wood_column(&mut self, i: usize, style: WoodStyle, post: bool) {
        let c = self.ring[i];
        let (base, mut h) = (self.ring_base(i), self.wall_h[i]);
        if style == WoodStyle::Palisade || post {
            if self.rng.roll(0.07) {
                h = self.rng.int(0, 1);
            }
            if style == WoodStyle::PostPlank {
                h += 1;
            }
            let w = self.mats.wood_at(&mut self.rng, 0.03);
            for k in 0..h {
                let m = self.mats.log(w, Axis::Y);
                self.put_at(c, base + k, &m);
            }
            let spike = if style == WoodStyle::Palisade {
                0.22
            } else {
                0.35
            };
            if h >= 2 && self.rng.roll(spike) {
                let m = self.mats.fence(w);
                self.put_at(c, base + h, &m);
            }
        } else {
            let ph = (h - if self.rng.roll(0.5) { 1 } else { 0 }).max(1);
            let w = self.mats.wood_at(&mut self.rng, 0.04);
            for k in 0..ph {
                if k == 0 || !self.rng.roll(0.05) {
                    let m = self.mats.block(w);
                    self.put_at(c, base + k, &m);
                }
            }
            let r = self.rng.unit();
            if r < 0.35 {
                let m = self.mats.slab(w, Half::Bottom);
                self.put_at(c, base + ph, &m);
            } else if r < 0.47 {
                let t = self.tangent(i);
                let m = self.mats.stairs(w, t, Half::Bottom);
                self.put_at(c, base + ph, &m);
            }
        }
    }

    /// Stairs propped against the inside foot of the wall.
    fn buttresses(&mut self) {
        for i in 0..self.ring_len() {
            if self.fort[i] > 0 || self.in_gate[i] || self.wall_h[i] < 2 || !self.rng.roll(0.07) {
                continue;
            }
            let c = self.ring[i];
            for d in Dir::ALL {
                let n = d.step(c, 1);
                let r = self.resv.get(n);
                if !self.inside(n) || self.depth(n) != 1 || r == Res::Gate || r == Res::Path {
                    continue;
                }
                let y = self.g(n) + 1;
                if self.plan.get([n[0], y, n[1]]).is_none() && self.plan.occupied([c[0], y, c[1]]) {
                    let family = if self.mats.stone {
                        self.mats.stone_at(&mut self.rng, [n[0], y, n[1]])
                    } else {
                        self.mats.wood_at(&mut self.rng, 0.05)
                    };
                    let m = self.mats.stairs(family, d, Half::Bottom);
                    self.put_at(n, y, &m);
                }
                break;
            }
        }
    }

    fn walk_y(&self, i: usize, fort_h: i32) -> i32 {
        self.ring_base(i) + fort_h - 1 - self.fort_cut[i]
    }

    /// Thick wall sections: stone ramparts with a walkway and merlons, or a tall palisade with a
    /// plank walkway on stilts. Some sections have collapsed into a notch.
    pub(super) fn build_fortress(&mut self) -> i32 {
        let len = self.ring_len();
        let fort_h = self.rng.int(4, 5);
        let rail = self.rng.roll(0.4);
        for a in 0..self.fort_arcs.len() {
            let (full, center, span) = (
                self.fort_arcs[a].full,
                self.fort_arcs[a].center,
                self.fort_arcs[a].span,
            );
            if full || !self.rng.roll(0.45) {
                continue;
            }
            let shift = self.rng.int(-span + 2, span - 2);
            let c = self.wrap(center as i64 + shift as i64);
            if self.in_gate[c] {
                continue;
            }
            for (j, v) in [1, 2, 3, 2, 1].into_iter().enumerate() {
                let k = self.wrap(c as i64 + j as i64 - 2);
                let cut = v + self.rng.int(0, 1);
                self.fort_cut[k] = self.fort_cut[k].max(cut);
            }
            self.rubble.push(Rubble {
                at: self.ring[c],
                r: 3.0,
                n: self.rng.int(5, 9),
                kind: RubbleKind::Camp,
            });
        }
        for i in 0..len {
            if self.fort[i] == 0 || self.in_gate[i] {
                continue;
            }
            let c = self.ring[i];
            let base = self.ring_base(i);
            if self.mats.stone {
                let top = base + fort_h - self.fort_cut[i];
                for y in base..=top {
                    let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                    let m = self.mats.block(s);
                    self.put_at(c, y, &m);
                }
                if i % 2 == 0 && self.fort_cut[i] == 0 {
                    let r = self.rng.unit();
                    let s = self.mats.stone_at(&mut self.rng, [c[0], top + 1, c[1]]);
                    if r < 0.62 {
                        let m = self.mats.block(s);
                        self.put_at(c, top + 1, &m);
                    } else if r < 0.78 {
                        let m = self.mats.slab(s, Half::Bottom);
                        self.put_at(c, top + 1, &m);
                    }
                }
            } else {
                let mut top = base + fort_h - self.fort_cut[i];
                if self.rng.roll(0.3) {
                    top += 1;
                }
                if self.rng.roll(0.12) {
                    top -= 1;
                }
                let w = self.mats.wood_at(&mut self.rng, 0.03);
                for y in base..=top {
                    let m = self.mats.log(w, Axis::Y);
                    self.put_at(c, y, &m);
                }
                if self.fort_cut[i] == 0 && self.rng.roll(0.25) {
                    let m = self.mats.fence(w);
                    self.put_at(c, top + 1, &m);
                }
            }
            let inner = self.inner_by_src.get(&i).cloned().unwrap_or_default();
            for ci in inner {
                if self.depth(ci) >= self.fort[i] {
                    continue;
                }
                let (own, wy) = (self.g(ci) + 1, self.walk_y(i, fort_h));
                if self.mats.stone {
                    for y in own..=wy {
                        if y < wy || !self.rng.roll(0.03) {
                            let s = self.mats.stone_at(&mut self.rng, [ci[0], y, ci[1]]);
                            let m = self.mats.block(s);
                            self.put_at(ci, y, &m);
                        }
                    }
                } else {
                    if i % 3 == 0 {
                        for y in own..wy {
                            let m = self.mats.log(self.mats.wood, Axis::Y);
                            self.put_at(ci, y, &m);
                        }
                    }
                    let r = self.rng.unit();
                    if r < 0.04 {
                        continue;
                    }
                    let w = self.mats.wood_at(&mut self.rng, 0.05);
                    let deck = if r < 0.13 {
                        self.mats.slab(w, Half::Top)
                    } else {
                        self.mats.block(w)
                    };
                    self.put_at(ci, wy, &deck);
                    if rail && self.depth(ci) == self.fort[i] - 1 && self.rng.roll(0.75) {
                        let m = self.mats.fence(self.mats.wood);
                        self.put_at(ci, wy + 1, &m);
                    }
                }
            }
        }
        self.fortress_ladders(fort_h);
        fort_h
    }

    fn fortress_ladders(&mut self, fort_h: i32) {
        let len = self.ring_len();
        for a in 0..self.fort_arcs.len() {
            let (full, center, span) = (
                self.fort_arcs[a].full,
                self.fort_arcs[a].center,
                self.fort_arcs[a].span,
            );
            let want = if full {
                ((len as f32 / 30.0).round() as i32).max(2)
            } else {
                self.rng.int(1, 2)
            };
            let mut made = 0;
            for _ in 0..40 {
                if made >= want {
                    break;
                }
                let j = if full {
                    self.rng.int(0, len as i32 - 1) as usize
                } else {
                    let shift = self.rng.int(-span, span);
                    self.wrap(center as i64 + shift as i64)
                };
                if self.in_gate[j] || self.fort[j] == 0 || self.fort_cut[j] > 0 {
                    continue;
                }
                let cand: Vec<[i32; 2]> = self
                    .inner_by_src
                    .get(&j)
                    .map(|v| {
                        v.iter()
                            .copied()
                            .filter(|&c| self.depth(c) == self.fort[j] - 1)
                            .collect()
                    })
                    .unwrap_or_default();
                if cand.is_empty() {
                    continue;
                }
                let f = *self.rng.pick(&cand);
                let mut dirs = Dir::ALL;
                self.rng.shuffle(&mut dirs);
                for d in dirs {
                    let n = d.step(f, 1);
                    if !self.inside(n)
                        || self.depth(n) != self.fort[j]
                        || self.resv.get(n) != Res::Free
                    {
                        continue;
                    }
                    let wy = self.walk_y(j, fort_h);
                    if !self.mats.stone {
                        for y in self.g(f) + 1..wy {
                            let m = self.mats.log(self.mats.wood, Axis::Y);
                            self.put_at(f, y, &m);
                        }
                    }
                    for y in self.g(n) + 1..=wy {
                        self.put_at(n, y, &decor!("ladder").facing(d));
                    }
                    self.resv.set(n, Res::Access);
                    made += 1;
                    break;
                }
            }
        }
    }

    pub(super) fn fortress_top(&self, i: usize, fort_h: i32) -> (i32, i32) {
        (self.ring_base(i) + fort_h, self.walk_y(i, fort_h))
    }

    pub(super) fn ring_floor(&self, i: usize) -> i32 {
        self.ring_base(i)
    }
}
