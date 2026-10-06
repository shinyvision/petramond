use mod_sdk::build::{Axis, Dir, Draw, Half};

use super::build::Builder;
use super::layout::Rails;
use super::Res;

impl Builder<'_> {
    /// A way up every plateau from inside the camp: a stair run down its face, or a ladder.
    pub(super) fn build_access(&mut self) {
        let layout = self.layout;
        for plateau in &layout.plateaus {
            let mut cand: Vec<(f32, [i32; 2], Dir)> = Vec::new();
            for &c in &plateau.cells {
                if !self.outline.inside(c) {
                    continue;
                }
                for d in Dir::ALL {
                    let n = d.step(c, 1);
                    let r = self.resv(n);
                    if self.layout.plateau_at.get(n) > 0
                        || !self.outline.inside(n)
                        || self.outline.depth(n) < 2
                        || (r != Res::Free && r != Res::Path)
                        || self.g(c) - self.g(n) < 3
                        || self.plan.get([n[0], self.g(n) + 1, n[1]]).is_some()
                        || self.plan.get([c[0], self.g(c) + 1, c[1]]).is_some()
                    {
                        continue;
                    }
                    let dist = ((n[0] - self.outline.center[0]) as f32)
                        .hypot((n[1] - self.outline.center[1]) as f32);
                    cand.push((dist, c, d));
                }
            }
            if cand.is_empty() {
                continue;
            }
            for entry in cand.iter_mut() {
                entry.0 += self.rng.range(0.0, 6.0);
            }
            cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let (_, c, d) = cand[0];
            let top = self.g(c);
            if self.rng.roll(0.55) && self.stair_run(c, d, top) {
                continue;
            }
            let n = d.step(c, 1);
            for y in self.g(n) + 1..=top {
                self.put_at(n, y, &decor!("ladder").facing(d));
            }
            self.take_access(n);
        }
    }

    fn stair_run(&mut self, c: [i32; 2], d: Dir, top: i32) -> bool {
        let mut steps = Vec::new();
        for k in 0.. {
            let s = d.step(c, k + 1);
            let y = top - k;
            if !self.ground.contains(s) {
                return false;
            }
            if y <= self.g(s) {
                break;
            }
            let r = self.resv(s);
            if !self.outline.inside(s)
                || self.layout.plateau_at.get(s) > 0
                || (r != Res::Free && r != Res::Path)
                || self.plan.get([s[0], y, s[1]]).is_some()
            {
                return false;
            }
            steps.push((s, y));
        }
        if steps.is_empty() {
            return false;
        }
        let wood = self.mats.wood;
        for (s, y) in steps {
            let family = if self.mats.stone {
                self.mats.stone_at(&mut self.rng, [s[0], y, s[1]])
            } else {
                self.mats.wood_at(&mut self.rng, 0.05)
            };
            let m = self.mats.stairs(family, d, Half::Bottom);
            self.put_at(s, y, &m);
            for yy in self.g(s) + 1..y {
                let m = if self.mats.stone {
                    let f = self.mats.stone_at(&mut self.rng, [s[0], yy, s[1]]);
                    self.mats.block(f)
                } else if self.rng.roll(0.3) {
                    self.mats.log(wood, Axis::Y)
                } else {
                    self.mats.block(wood)
                };
                self.put_at(s, yy, &m);
            }
            self.take_access(s);
        }
        true
    }

    /// Decks between plateau tops, rising in half steps, railed and holed, on posts.
    pub(super) fn build_bridges(&mut self) {
        let layout = self.layout;
        let wood = self.mats.wood;
        for br in &layout.bridges {
            let mut deck_at = mod_sdk::FxHashMap::default();
            for cell in &br.span.cells {
                let c = cell.pos;
                let full = cell.level.fract() == 0.0 || cell.edge;
                let y = if full {
                    cell.level.ceil() as i32 - 1
                } else {
                    cell.level.floor() as i32
                };
                deck_at.insert(c, y);
                if self.resv(c) == Res::Centre && self.plan.get([c[0], y, c[1]]).is_some() {
                    continue;
                }
                let on_plateau = self.layout.plateau_at.get(c) > 0;
                for yy in y..=y + 3 {
                    if !(on_plateau && yy <= self.g(c)) && self.plan.get([c[0], yy, c[1]]).is_some()
                    {
                        self.air(c, yy);
                    }
                }
                if (!cell.edge && self.rng.roll(0.05)) || (cell.edge && self.rng.roll(0.08)) {
                    continue;
                }
                let m = if self.mats.stone {
                    let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                    if full {
                        self.mats.block(s)
                    } else {
                        self.mats.slab(s, Half::Bottom)
                    }
                } else {
                    let w = self.mats.wood_at(&mut self.rng, 0.06);
                    if full {
                        self.mats.block(w)
                    } else {
                        self.mats.slab(w, Half::Bottom)
                    }
                };
                self.put_at(c, y, &m);
                let railed = match br.rails {
                    Rails::Both => true,
                    Rails::One => cell.side > 0,
                    Rails::Neither => false,
                };
                let on_top = on_plateau
                    && self.g(c)
                        == self.layout.plateaus[self.layout.plateau_at.get(c) as usize - 1].top;
                if cell.edge && railed && !on_top && self.rng.roll(0.72) {
                    let m = if self.mats.stone {
                        let s = self.mats.stone_at(&mut self.rng, [c[0], y + 1, c[1]]);
                        self.mats.slab(s, Half::Bottom)
                    } else {
                        self.mats.fence(wood)
                    };
                    self.put_at(c, y + 1, &m);
                }
            }
            for &(c, level) in &br.span.supports {
                let r = self.resv(c);
                if !self.ground.contains(c) || r == Res::Centre || r == Res::Hut {
                    continue;
                }
                let y = deck_at.get(&c).copied().unwrap_or(level.ceil() as i32 - 1);
                let g = self.g(c);
                if y - g < 2 {
                    continue;
                }
                if self.rng.roll(0.08) {
                    let m = if self.mats.stone {
                        let s = self.mats.stone_at(&mut self.rng, [c[0], g + 1, c[1]]);
                        self.mats.block(s)
                    } else {
                        self.mats.log(wood, Axis::Y)
                    };
                    self.put_at(c, g + 1, &m);
                    continue;
                }
                for yy in g + 1..y {
                    let m = if self.mats.stone {
                        let s = self.mats.stone_at(&mut self.rng, [c[0], yy, c[1]]);
                        self.mats.block(s)
                    } else {
                        let w = self.mats.wood_at(&mut self.rng, 0.04);
                        self.mats.log(w, Axis::Y)
                    };
                    self.put_at(c, yy, &m);
                }
                if !deck_at.contains_key(&c) {
                    let m = if self.mats.stone {
                        let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                        self.mats.block(s)
                    } else {
                        self.mats.block(wood)
                    };
                    self.put_at(c, y, &m);
                }
                if !self.mats.stone && self.rng.roll(0.4) {
                    let m = self.mats.log(wood, Axis::Y);
                    self.put_at(c, y + 1, &m);
                }
            }
        }
    }
}
