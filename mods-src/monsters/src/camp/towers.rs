use mod_sdk::build::{Axis, Draw, Frame, Half};

use super::ground::{Rubble, RubbleKind};
use super::Camp;

impl Camp<'_> {
    pub(super) fn build_towers(&mut self) {
        for t in 0..self.towers.len() {
            let (s, min, dir) = (self.towers[t].s, self.towers[t].min, self.towers[t].dir);
            let frame = Frame::new(min, s, s, dir);
            let mut base = i32::MIN;
            for lz in 0..s {
                for lx in 0..s {
                    let c = frame.at(lx, lz);
                    base = base.max(self.g(c) + 1);
                    let floor = self.g(c) + 1;
                    for y in floor..floor + 16 {
                        if self.plan.get([c[0], y, c[1]]).is_some() {
                            self.plan.unset([c[0], y, c[1]]);
                        }
                    }
                }
            }
            let h = self.rng.int(5, 8);
            if self.mats.stone {
                self.stone_tower(&frame, s, base, base + h);
            } else {
                self.wood_tower(&frame, s, base, base + h);
            }
        }
    }

    fn stone_tower(&mut self, frame: &Frame, s: i32, base: i32, plat: i32) {
        let corner = |lx: i32, lz: i32| (lx == 0 || lx == s - 1) && (lz == 0 || lz == s - 1);
        let perim = |lx: i32, lz: i32| lx == 0 || lz == 0 || lx == s - 1 || lz == s - 1;
        let stone = |camp: &mut Camp<'_>, c: [i32; 2], y: i32| {
            let f = camp.mats.stone_at(&mut camp.rng, [c[0], y, c[1]]);
            camp.mats.block(f)
        };
        for lz in 0..s {
            for lx in 0..s {
                let c = frame.at(lx, lz);
                for y in self.g(c) + 1..base {
                    let m = stone(self, c, y);
                    self.put_at(c, y, &m);
                }
                if perim(lx, lz) {
                    for y in base..plat {
                        if lz == s - 1 && lx == 1 && y < base + 2 {
                            continue;
                        }
                        if !corner(lx, lz)
                            && !(lx == 1 && lz == 0)
                            && y >= base + 2
                            && self.rng.roll(0.1)
                        {
                            continue;
                        }
                        let m = stone(self, c, y);
                        self.put_at(c, y, &m);
                    }
                }
                if lx == 1 && lz == 1 {
                    for y in base..=plat {
                        self.put_at(c, y, &decor!("ladder").facing(frame.fwd()));
                    }
                } else if corner(lx, lz) || !self.rng.roll(0.05) {
                    let m = stone(self, c, plat);
                    self.put_at(c, plat, &m);
                }
                if perim(lx, lz) {
                    if corner(lx, lz) {
                        let m = stone(self, c, plat + 1);
                        self.put_at(c, plat + 1, &m);
                        if self.rng.roll(0.45) {
                            let f = self.mats.stone_at(&mut self.rng, [c[0], plat + 2, c[1]]);
                            let m = if self.rng.roll(0.5) {
                                self.mats.block(f)
                            } else {
                                self.mats.slab(f, Half::Bottom)
                            };
                            self.put_at(c, plat + 2, &m);
                        }
                    } else {
                        let merlon = (lx + lz) % 2 == 0;
                        if self.rng.roll(if merlon { 0.75 } else { 0.25 }) {
                            let f = self.mats.stone_at(&mut self.rng, [c[0], plat + 1, c[1]]);
                            let m = if merlon {
                                self.mats.block(f)
                            } else {
                                self.mats.slab(f, Half::Bottom)
                            };
                            self.put_at(c, plat + 1, &m);
                        }
                    }
                }
            }
        }
        if self.rng.roll(0.35) {
            let (cx, cz) = (*self.rng.pick(&[0, s - 1]), *self.rng.pick(&[0, s - 1]));
            for lz in 0..s {
                for lx in 0..s {
                    let d = (lx - cx).abs() + (lz - cz).abs();
                    if d > 1 {
                        continue;
                    }
                    let c = frame.at(lx, lz);
                    self.plan.unset([c[0], plat + 1, c[1]]);
                    self.plan.unset([c[0], plat + 2, c[1]]);
                    if d == 0 {
                        self.plan.unset([c[0], plat - 1, c[1]]);
                    }
                }
            }
            self.rubble.push(Rubble {
                at: frame.at(cx, cz),
                r: 3.0,
                n: self.rng.int(3, 5),
                kind: RubbleKind::Camp,
            });
        }
    }

    fn wood_tower(&mut self, frame: &Frame, s: i32, base: i32, plat: i32) {
        let corner = |lx: i32, lz: i32| (lx == 0 || lx == s - 1) && (lz == 0 || lz == s - 1);
        let perim = |lx: i32, lz: i32| lx == 0 || lz == 0 || lx == s - 1 || lz == s - 1;
        let roof = self.rng.roll(0.6);
        let leg_top = plat + if roof { 3 } else { 1 };
        let wood = self.mats.wood;
        for (lx, lz) in [(0, 0), (s - 1, 0), (0, s - 1), (s - 1, s - 1)] {
            let c = frame.at(lx, lz);
            let w = self.mats.wood_at(&mut self.rng, 0.04);
            for y in self.g(c) + 1..=leg_top {
                let m = self.mats.log(w, Axis::Y);
                self.put_at(c, y, &m);
            }
            if !roof && self.rng.roll(0.25) {
                self.put_at(c, leg_top + 1, &decor!("torch"));
            }
        }
        let (pole, hatch) = (frame.at(1, 1), frame.at(1, 2));
        for y in self.g(pole) + 1..plat {
            let m = self.mats.log(wood, Axis::Y);
            self.put_at(pole, y, &m);
        }
        for y in self.g(hatch) + 1..=plat {
            self.put_at(hatch, y, &decor!("ladder").facing(frame.fwd()));
        }
        let brace_y = base + (plat - base) / 2;
        // Braces on the back and both flanks; the camp side stays open for the ladder.
        for side in 0..3 {
            if !self.rng.roll(0.6) {
                continue;
            }
            let axis = if side == 0 {
                frame.x_axis()
            } else {
                frame.z_axis()
            };
            for k in 1..s - 1 {
                let (lx, lz) = match side {
                    0 => (k, 0),
                    1 => (0, k),
                    _ => (s - 1, k),
                };
                if self.rng.roll(0.85) {
                    let c = frame.at(lx, lz);
                    let m = self.mats.log(wood, axis);
                    self.put_at(c, brace_y, &m);
                }
            }
        }
        for lz in 0..s {
            for lx in 0..s {
                if corner(lx, lz) || (lx == 1 && lz == 2) {
                    continue;
                }
                let r = self.rng.unit();
                if r < 0.05 && !(lx == 1 && lz == 1) {
                    continue;
                }
                let c = frame.at(lx, lz);
                let w = self.mats.wood_at(&mut self.rng, 0.05);
                let m = if r < 0.15 {
                    self.mats.slab(w, Half::Top)
                } else {
                    self.mats.block(w)
                };
                self.put_at(c, plat, &m);
                if perim(lx, lz) && self.rng.roll(0.7) {
                    let m = self.mats.fence(wood);
                    self.put_at(c, plat + 1, &m);
                }
            }
        }
        if roof {
            let ry = plat + 4;
            let sides = ["back", "fwd", "left", "right"];
            let collapse = self.rng.roll(0.3).then(|| *self.rng.pick(&sides));
            for lz in -1..=s {
                for lx in -1..=s {
                    let side = if lz == -1 {
                        Some(("back", frame.back()))
                    } else if lz == s {
                        Some(("fwd", frame.fwd()))
                    } else if lx == -1 {
                        Some(("left", frame.left()))
                    } else if lx == s {
                        Some(("right", frame.right()))
                    } else {
                        None
                    };
                    if collapse.is_some() && side.map(|s| s.0) == collapse {
                        continue;
                    }
                    if self.rng.roll(0.18) {
                        continue;
                    }
                    let c = frame.at(lx, lz);
                    let m = match side {
                        Some((_, d)) => self.mats.stairs(wood, d, Half::Bottom),
                        None => {
                            let w = self.mats.wood_at(&mut self.rng, 0.04);
                            self.mats.block(w)
                        }
                    };
                    self.put_at(c, ry, &m);
                }
            }
            for lz in 1..s - 1 {
                for lx in 1..s - 1 {
                    if self.rng.roll(0.8) {
                        let c = frame.at(lx, lz);
                        let m = self.mats.slab(wood, Half::Bottom);
                        self.put_at(c, ry + 1, &m);
                    }
                }
            }
            if let Some(side) = collapse {
                let at = match side {
                    "back" => frame.at(s / 2, -1),
                    "fwd" => frame.at(s / 2, s),
                    _ => frame.at(s / 2, s / 2),
                };
                self.rubble.push(Rubble {
                    at,
                    r: 2.5,
                    n: 3,
                    kind: RubbleKind::Wood,
                });
            }
        }
    }
}
