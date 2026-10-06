use mod_sdk::build::{
    a_frame, gable, lean_to, Axis, Dir, Draw, Frame, Half, Material, Name, RoofStyle,
};
use mod_sdk::GenRng;

use super::build::Builder;
use super::layout::HutKind;

/// Engine cell data naming the loot table a generated chest is stocked from when first opened.
const LOOT_DATA: &str = "petramond:loot";

/// Marks the chest at `pos` to be stocked from `table`.
pub(super) fn loot(plan: &mut mod_sdk::build::Plan, pos: [i32; 3], table: &str) {
    plan.data(
        pos,
        LOOT_DATA,
        format!("{{\"table\":\"{table}\"}}").into_bytes(),
    );
}

const DOOR: [[i32; 3]; 2] = [[0, 0, 0], [0, 1, 0]];

impl Builder<'_> {
    pub(super) fn build_huts(&mut self) {
        let layout = self.layout;
        for hut in &layout.huts {
            let (kind, frame, w, d) = (hut.kind, hut.frame, hut.w, hut.d);
            let mut base = i32::MIN;
            for lz in 0..d {
                for lx in 0..w {
                    base = base.max(self.g(frame.at(lx, lz)) + 1);
                }
            }
            let wood = if self.rng.roll(0.85) {
                self.mats.wood
            } else {
                self.mats.other_wood(&mut self.rng)
            };
            let cobble = self.mats.stone || self.rng.roll(0.5);
            for lz in 0..d {
                for lx in 0..w {
                    let c = frame.at(lx, lz);
                    for y in self.g(c) + 1..base {
                        let m = if cobble {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                            self.mats.block(s)
                        } else {
                            named!("dirt")
                        };
                        self.put_at(c, y, &m);
                    }
                }
            }
            match kind {
                HutKind::Hut => self.hut(&frame, w, d, base, wood),
                HutKind::LeanTo => self.lean_to(&frame, w, d, base, wood),
                HutKind::AFrame => self.tent(&frame, w, d, base, wood),
            }
        }
    }

    fn chest(&mut self, frame: &Frame, lx: i32, lz: i32, y: i32, facing: Dir) {
        let c = frame.at(lx, lz);
        self.put_at(c, y, &named!("chest").facing(facing));
        loot(&mut self.plan, [c[0], y, c[1]], crate::keys::CAMP_HUT_LOOT);
    }

    fn roof_style<'s>(
        &self,
        stairs: &'s dyn Fn(Dir) -> Material,
        family: Name,
        hole: f32,
    ) -> RoofStyle<'s> {
        RoofStyle {
            stairs,
            slab: self.mats.slab(family, Half::Bottom),
            hole,
        }
    }

    fn hut(&mut self, frame: &Frame, w: i32, d: i32, base: i32, wood: Name) {
        let wall_h = if self.rng.roll(0.75) { 3 } else { 2 };
        #[derive(Clone, Copy, PartialEq)]
        enum Walls {
            Stone,
            Planks,
            Logs,
        }
        let walls = if self.mats.stone {
            *self.rng.weighted(&[
                (Walls::Stone, 5.0),
                (Walls::Planks, 3.0),
                (Walls::Logs, 2.0),
            ])
        } else {
            *self.rng.weighted(&[
                (Walls::Planks, 4.0),
                (Walls::Logs, 3.0),
                (Walls::Stone, 1.0),
            ])
        };
        let door_x = (w - 1) / 2;
        let door = *self
            .rng
            .weighted(&[(Some(false), 4.0), (Some(true), 3.0), (None, 3.0)]);
        if self.rng.roll(0.5) {
            for lz in 1..d - 1 {
                for lx in 1..w - 1 {
                    let m = self.mats.block(wood);
                    let c = frame.at(lx, lz);
                    self.put_at(c, base - 1, &m);
                }
            }
        }
        for lz in 0..d {
            for lx in 0..w {
                if !(lx == 0 || lz == 0 || lx == w - 1 || lz == d - 1) {
                    continue;
                }
                let corner = (lx == 0 || lx == w - 1) && (lz == 0 || lz == d - 1);
                let c = frame.at(lx, lz);
                for k in 0..wall_h {
                    let y = base + k;
                    if lx == door_x && lz == d - 1 && k < 2 {
                        if let (0, Some(open), Some(m)) = (k, door, self.mats.door(wood)) {
                            self.plan.object(
                                [c[0], y, c[1]],
                                &m.facing(frame.fwd()).open(open),
                                &DOOR,
                            );
                        }
                        continue;
                    }
                    if corner {
                        let m = self.mats.log(wood, Axis::Y);
                        self.put_at(c, y, &m);
                        continue;
                    }
                    if k == 1 && (lx == 0 || lx == w - 1) && lz == d / 2 {
                        let r = self.rng.unit();
                        if r < 0.55 {
                            continue;
                        }
                        if r < 0.8 {
                            let m = self.mats.fence(wood);
                            self.put_at(c, y, &m);
                            continue;
                        }
                    }
                    if k >= 1 && self.rng.roll(0.07) {
                        continue;
                    }
                    let axis = if lz == 0 || lz == d - 1 {
                        frame.x_axis()
                    } else {
                        frame.z_axis()
                    };
                    let m = match walls {
                        Walls::Logs => self.mats.log(wood, axis),
                        Walls::Stone if k < 2 => {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                            self.mats.block(s)
                        }
                        _ => self.mats.block(wood),
                    };
                    self.put_at(c, y, &m);
                }
            }
        }
        let roof_family = if self.mats.stone && self.rng.roll(0.2) {
            self.mats.cobblestone()
        } else {
            wood
        };
        let stairs_of = |dir: Dir| self.mats.stairs(roof_family, dir, Half::Bottom);
        let style = self.roof_style(&stairs_of, roof_family, 0.07);
        let planks = self.mats.block(wood);
        let mut fill = |_: &mut GenRng| planks;
        gable(
            &mut self.plan,
            &mut self.rng,
            frame,
            [w, d],
            base + wall_h,
            &style,
            Some(&mut fill),
        );
        if self.rng.roll(0.22) && w > 2 {
            let a = self.rng.int(0, w - 2);
            let back = self.rng.roll(0.5);
            for lx in a..=a + 1 {
                for k in 0..d {
                    let lz = if back { k - 1 } else { d - k };
                    if (back && lz >= d / 2) || (!back && lz < (d + 1) / 2) {
                        continue;
                    }
                    let c = frame.at(lx, lz);
                    self.plan.unset([c[0], base + wall_h + k, c[1]]);
                }
            }
            let c = frame.at(a, d / 2);
            if self.plan.get([c[0], base, c[1]]).is_none() {
                let facing = *self.rng.pick(&Dir::ALL);
                let m = self.mats.stairs(roof_family, facing, Half::Bottom);
                self.put_at(c, base, &m);
            }
        }
        if self.rng.roll(0.55) && w > 3 {
            let lx = self.rng.int(1, w - 2);
            self.chest(frame, lx, 1, base, frame.fwd());
        }
        if self.rng.roll(0.12) && w > 4 {
            let c = frame.at(w - 2, d - 2);
            self.put_at(c, base, &named!("crafting_table"));
        }
    }

    fn lean_to(&mut self, frame: &Frame, w: i32, d: i32, base: i32, wood: Name) {
        for lx in 0..w {
            for k in 0..3 {
                if k > 0 && lx > 0 && lx < w - 1 && self.rng.roll(0.08) {
                    continue;
                }
                let m = if lx == 0 || lx == w - 1 {
                    self.mats.log(wood, Axis::Y)
                } else {
                    self.mats.block(wood)
                };
                let c = frame.at(lx, 0);
                self.put_at(c, base + k, &m);
            }
        }
        let fence_posts = !self.mats.stone && self.rng.roll(0.5);
        for lx in [0, w - 1] {
            for k in 0..2 {
                let m = if fence_posts {
                    self.mats.fence(wood)
                } else {
                    self.mats.log(wood, Axis::Y)
                };
                let c = frame.at(lx, d - 1);
                self.put_at(c, base + k, &m);
            }
        }
        if self.rng.roll(0.4) {
            for lx in [0, w - 1] {
                for lz in 1..d - 1 {
                    let m = self.mats.block(wood);
                    let c = frame.at(lx, lz);
                    self.put_at(c, base, &m);
                }
            }
        }
        let stairs_of = |dir: Dir| self.mats.stairs(wood, dir, Half::Bottom);
        let style = self.roof_style(&stairs_of, wood, 0.08);
        lean_to(
            &mut self.plan,
            &mut self.rng,
            frame,
            [w, d],
            (base + 4) as f32,
            &style,
        );
        if self.rng.roll(0.4) {
            self.chest(frame, (w - 1) / 2, 1, base, frame.fwd());
        }
    }

    fn tent(&mut self, frame: &Frame, w: i32, d: i32, base: i32, wood: Name) {
        let stairs_of = |dir: Dir| self.mats.stairs(wood, dir, Half::Bottom);
        let style = self.roof_style(&stairs_of, wood, 0.06);
        let back = self.mats.block(wood);
        a_frame(
            &mut self.plan,
            &mut self.rng,
            frame,
            [w, d],
            base,
            &style,
            Some(&back),
        );
    }
}
