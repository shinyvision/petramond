use mod_sdk::build::{Axis, Dir, Draw, Half};

use super::Camp;

impl Camp<'_> {
    fn clear_above(&mut self, c: [i32; 2], from: i32) {
        for y in from..from + 12 {
            if self.plan.get([c[0], y, c[1]]).is_some() {
                self.air(c, y);
            }
        }
    }

    /// Openings through the wall (and through fortress sections), framed by posts under a
    /// lintel that has partly fallen in; stone gates get a rough arch.
    pub(super) fn build_gates(&mut self, fort_h: i32) {
        for gi in 0..self.gates.len() {
            let idx = self.gates[gi].idx.clone();
            let (from, to) = (self.gates[gi].from, self.gates[gi].to);
            let cells: Vec<[i32; 2]> = idx.iter().map(|&j| self.ring[j]).collect();
            let mut inner: Vec<[i32; 2]> = Vec::new();
            for &j in &idx {
                if let Some(v) = self.inner_by_src.get(&j) {
                    inner.extend(v.iter().copied().filter(|&c| self.depth(c) < self.fort[j]));
                }
            }
            for &c in cells.iter().chain(&inner) {
                let floor = self.g(c) + 1;
                self.clear_above(c, floor);
            }
            let floor_y = cells.iter().map(|&c| self.g(c) + 1).max().unwrap_or(0);
            let lintel_y = floor_y + self.rng.int(3, 4);
            let fort = idx.iter().any(|&j| self.fort[j] > 0);
            let (left, right) = (self.wrap(from as i64 - 1), self.wrap(to as i64 + 1));
            let axis = {
                let (l, r) = (self.ring[left], self.ring[right]);
                if (r[0] - l[0]).abs() >= (r[1] - l[1]).abs() {
                    Axis::X
                } else {
                    Axis::Z
                }
            };
            let wood = self.mats.wood;
            if !fort {
                for post in [left, right] {
                    let c = self.ring[post];
                    let base = self.ring_floor(post);
                    let top = lintel_y + 1;
                    self.clear_above(c, base);
                    for y in base..=top {
                        let m = if self.mats.stone {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                            self.mats.block(s)
                        } else {
                            self.mats.log(wood, Axis::Y)
                        };
                        self.put_at(c, y, &m);
                    }
                    let r = self.rng.unit();
                    if r < 0.3 {
                        self.put_at(c, top + 1, &decor!("torch"));
                    } else if r < 0.55 {
                        let m = if self.mats.stone {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], top + 1, c[1]]);
                            self.mats.slab(s, Half::Bottom)
                        } else {
                            self.mats.fence(wood)
                        };
                        self.put_at(c, top + 1, &m);
                    }
                }
            }
            let n = cells.len();
            // A broken lintel has fallen from one end, so what remains still rests on a post.
            let fallen = if !fort && n > 1 && self.rng.roll(0.4) {
                self.rng.int(1, n as i32 - 1) as usize
            } else {
                0
            };
            let from_left = self.rng.roll(0.5);
            let standing = |k: usize| {
                if from_left {
                    k >= fallen
                } else {
                    k < n - fallen
                }
            };
            for (k, &c) in cells.iter().enumerate() {
                if fort {
                    let (top, _) = self.fortress_top(idx[k], fort_h);
                    for y in lintel_y..=top {
                        let m = if self.mats.stone {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                            self.mats.block(s)
                        } else {
                            self.mats.log(wood, axis)
                        };
                        self.put_at(c, y, &m);
                    }
                } else if standing(k) {
                    let m = if self.mats.stone {
                        let s = self.mats.stone_at(&mut self.rng, [c[0], lintel_y, c[1]]);
                        self.mats.block(s)
                    } else {
                        self.mats.log(wood, axis)
                    };
                    self.put_at(c, lintel_y, &m);
                }
                let edge = k == 0 || k == n - 1;
                if self.mats.stone && edge && n > 1 && standing(k) && self.rng.roll(0.8) {
                    let other = cells[if k == 0 { 1 } else { n - 2 }];
                    let facing = Dir::of((other[0] - c[0]) as f32, (other[1] - c[1]) as f32);
                    let s = self
                        .mats
                        .stone_at(&mut self.rng, [c[0], lintel_y - 1, c[1]]);
                    let m = self.mats.stairs(s, facing, Half::Top);
                    self.put_at(c, lintel_y - 1, &m);
                }
                if !self.mats.stone && !edge && standing(k) && self.rng.roll(0.3) {
                    let m = self.mats.fence(wood);
                    self.put_at(c, lintel_y - 1, &m);
                    if lintel_y - 2 >= floor_y + 2 && self.rng.roll(0.35) {
                        self.put_at(c, lintel_y - 2, &m);
                    }
                }
                if self.mats.stone && !fort && standing(k) && self.rng.roll(0.4) {
                    let s = self
                        .mats
                        .stone_at(&mut self.rng, [c[0], lintel_y + 1, c[1]]);
                    let m = self.mats.slab(s, Half::Bottom);
                    self.put_at(c, lintel_y + 1, &m);
                }
            }
            if fort {
                for c in inner {
                    let src = self.field.source(c).unwrap_or(idx[0]);
                    let (_, wy) = self.fortress_top(src, fort_h);
                    for y in lintel_y..=wy {
                        let m = if self.mats.stone {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                            self.mats.block(s)
                        } else {
                            self.mats.block(wood)
                        };
                        self.put_at(c, y, &m);
                    }
                }
            }
        }
    }
}
