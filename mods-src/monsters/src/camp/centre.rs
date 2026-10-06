use mod_sdk::build::{gable, Axis, Dir, Draw, Frame, Half, Material, Noise2, RoofStyle};

use super::build::Builder;
use super::flag::Seat;
use super::ground::{Rubble, RubbleKind};
use super::huts::loot;
use super::layout::{Centre, CentreKind};
use super::style::named;

impl Builder<'_> {
    /// Builds the centrepiece, if the camp has one; returns where its flag stands, if it flies
    /// one. `tower_tops` are the built towers' platform floors.
    pub(super) fn build_centre(&mut self, tower_tops: &[i32]) -> Option<Seat> {
        let layout = self.layout;
        let centre = layout.centre.as_ref()?;
        let at = centre.at;
        let face = match layout.gates.first() {
            Some(g) => {
                let c = self.outline.ring[g.i];
                Dir::of((c[0] - at[0]) as f32, (c[1] - at[1]) as f32)
            }
            None => *self.rng.pick(&Dir::ALL),
        };
        match centre.kind {
            CentreKind::Well => self.well(at, face),
            CentreKind::Statue => self.statue(at, face, tower_tops),
            CentreKind::Arena => self.arena(centre),
        }
    }

    /// Returns the flag's seat on the roof's ridge.
    fn well(&mut self, at: [i32; 2], face: Dir) -> Option<Seat> {
        let ws = if self.rng.roll(0.55) { 4 } else { 3 };
        let frame = Frame::new([at[0] - ws / 2, at[1] - ws / 2], ws, ws, face);
        let mut base = i32::MIN;
        for lz in 0..ws {
            for lx in 0..ws {
                base = base.max(self.g(frame.at(lx, lz)));
            }
        }
        for lz in 0..ws {
            for lx in 0..ws {
                let c = frame.at(lx, lz);
                let rim = lx == 0 || lz == 0 || lx == ws - 1 || lz == ws - 1;
                for y in self.g(c) + 1..=base {
                    self.put_at(c, y, &named!("cobblestone"));
                }
                for y in base - 4..=base {
                    let m = if rim {
                        let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                        self.mats.block(s)
                    } else if y == base - 4 {
                        named!("gravel")
                    } else {
                        named!("water")
                    };
                    self.put_at(c, y, &m);
                }
                if rim {
                    let r = self.rng.unit();
                    let s = self.mats.stone_at(&mut self.rng, [c[0], base + 1, c[1]]);
                    if r < 0.72 {
                        let m = self.mats.block(s);
                        self.put_at(c, base + 1, &m);
                    } else if r < 0.93 {
                        let m = self.mats.slab(s, Half::Bottom);
                        self.put_at(c, base + 1, &m);
                    }
                }
            }
        }
        let wood = self.mats.wood;
        for (lx, lz) in [(0, 0), (ws - 1, 0), (0, ws - 1), (ws - 1, ws - 1)] {
            let top = if self.rng.roll(0.2) {
                base + 2
            } else {
                base + 4
            };
            let post = if self.mats.stone || self.rng.roll(0.5) {
                self.mats.fence(wood)
            } else {
                self.mats.log(wood, Axis::Y)
            };
            let c = frame.at(lx, lz);
            for y in base + 2..=top {
                self.put_at(c, y, &post);
            }
        }
        let stairs_of = |dir: Dir| self.mats.stairs(wood, dir, Half::Bottom);
        let style = RoofStyle {
            stairs: &stairs_of,
            slab: self.mats.slab(wood, Half::Bottom),
            hole: 0.12,
        };
        gable(
            &mut self.plan,
            &mut self.rng,
            &frame,
            [ws, ws],
            base + 4,
            &style,
            None,
        );
        if !self.layout.flagged {
            return None;
        }
        // The ridge's middle, made a full block so the pole stands on it, not half above it.
        let ridge = frame.at(ws / 2, (ws - 1) / 2);
        let top = (base + 4..=base + 4 + ws)
            .rev()
            .find(|&y| self.plan.occupied([ridge[0], y, ridge[1]]))
            .unwrap_or(base + 4 + (ws - 1) / 2);
        let cap = self.mats.block(wood);
        self.put_at(ridge, top, &cap);
        Some(Seat::Top(ridge, top, top))
    }

    /// Returns the flag's seat on a tower's corner, or on the statue without a tower.
    fn statue(&mut self, at: [i32; 2], face: Dir, tower_tops: &[i32]) -> Option<Seat> {
        let frame = Frame::new([at[0] - 2, at[1] - 2], 5, 5, face);
        let mut base = i32::MIN;
        for lz in 0..5 {
            for lx in 0..5 {
                base = base.max(self.g(frame.at(lx, lz)) + 1);
            }
        }
        let totem = !self.mats.stone && self.rng.roll(0.55);
        let plinth = if totem { "cobblestone" } else { "stone_bricks" };
        for lz in 0..5 {
            for lx in 0..5 {
                let c = frame.at(lx, lz);
                for y in self.g(c) + 1..base {
                    self.put_at(c, y, &named!("cobblestone"));
                }
                if lx == 0 || lz == 0 || lx == 4 || lz == 4 {
                    let side = if lz == 0 {
                        frame.back()
                    } else if lz == 4 {
                        frame.fwd()
                    } else if lx == 0 {
                        frame.left()
                    } else {
                        frame.right()
                    };
                    if !self.rng.roll(0.1) {
                        let m = self.mats.stairs(plinth, side, Half::Bottom);
                        self.put_at(c, base, &m);
                    }
                } else {
                    let m = self.mats.block(plinth);
                    self.put_at(c, base, &m);
                    let top = if totem {
                        self.mats.slab(plinth, Half::Bottom)
                    } else if self.rng.roll(0.8) {
                        self.mats.block("stone_bricks")
                    } else {
                        named!("polished_marble")
                    };
                    self.put_at(c, base + 1, &top);
                }
            }
        }
        if totem {
            self.totem(&frame, base + 1);
        } else {
            self.skeleton_figure(&frame, base + 2);
        }
        if !self.layout.flagged {
            return None;
        }
        self.tower_seat(tower_tops)
            .or(Some(Seat::Top(at, base, base + 12)))
    }

    fn totem(&mut self, frame: &Frame, y0: i32) {
        let wood = self.mats.wood_at(&mut self.rng, 0.1);
        let h = self.rng.int(7, 9);
        let set = |camp: &mut Builder<'_>, lx: i32, lz: i32, y: i32, m: &Material| {
            let c = frame.at(lx, lz);
            camp.put_at(c, y, m);
        };
        for y in y0..y0 + h {
            let m = self.mats.log(wood, Axis::Y);
            set(self, 2, 2, y, &m);
        }
        let bar = y0 + h - 3;
        for lx in [0, 1, 3, 4] {
            if lx == 1 || lx == 3 || self.rng.roll(0.8) {
                let m = self.mats.log(wood, frame.x_axis());
                set(self, lx, 2, bar, &m);
            }
        }
        set(self, 2, 2, y0 + h, &named!("calcite"));
        let m = self.mats.stairs(wood, frame.back(), Half::Top);
        set(self, 2, 1, y0 + h - 1, &m);
        let m = self.mats.stairs(wood, frame.fwd(), Half::Top);
        set(self, 2, 3, y0 + h - 1, &m);
        for lx in [0, 4] {
            if self.rng.roll(0.6) {
                let m = self.mats.fence(wood);
                set(self, lx, 2, bar - 1, &m);
            }
        }
        if self.rng.roll(0.5) {
            let m = self.mats.fence(wood);
            set(self, 2, 2, y0 + h + 1, &m);
        }
    }

    /// A crude skeleton on the plinth; sometimes it has lost an arm, or its skull has fallen.
    fn skeleton_figure(&mut self, frame: &Frame, y: i32) {
        let fig = *self.rng.pick(&["calcite", "marble", "polished_marble"]);
        let stairs_family = if fig == "calcite" {
            "polished_marble"
        } else {
            fig
        };
        let body = if fig == "calcite" {
            named!("calcite")
        } else {
            self.mats.block(fig)
        };
        let lz = 2;
        let set = |camp: &mut Builder<'_>, lx: i32, dy: i32, m: &Material| {
            let c = frame.at(lx, lz);
            camp.put_at(c, y + dy, m);
        };
        for (lx, dy) in [
            (1, 0),
            (3, 0),
            (1, 1),
            (3, 1),
            (1, 2),
            (2, 2),
            (3, 2),
            (2, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ] {
            set(self, lx, dy, &body);
        }
        let rib = |camp: &Builder<'_>, d: Dir| camp.mats.stairs(stairs_family, d, Half::Top);
        let (left, right) = (rib(self, frame.left()), rib(self, frame.right()));
        set(self, 1, 3, &left);
        set(self, 3, 3, &right);
        let lost = self.rng.roll(0.3).then(|| *self.rng.pick(&[0, 4]));
        let raised = self.rng.roll(0.45);
        if lost != Some(0) {
            set(self, 0, 4, &body);
            set(self, 0, 3, &body);
        }
        if lost != Some(4) {
            set(self, 4, 4, &body);
            if raised {
                set(self, 4, 5, &body);
                set(self, 4, 6, &body);
                let sword = self.mats.fence(self.mats.wood);
                set(self, 4, 7, &sword);
            } else {
                set(self, 4, 3, &body);
            }
        }
        if self.rng.roll(0.35) {
            let c = frame.at(*self.rng.pick(&[-1, 5]), self.rng.int(0, 4));
            if self.ground.contains(c) {
                let g = self.g(c);
                self.put_at(c, g + 1, &named!("calcite"));
            }
        } else {
            set(self, 2, 5, &named!("calcite"));
            if self.rng.roll(0.3) {
                let m = self.mats.slab(stairs_family, Half::Bottom);
                set(self, 2, 6, &m);
            }
        }
        if let Some(lx) = lost {
            let at = frame.at(lx, 2);
            let block = if fig == "calcite" { "calcite" } else { fig };
            self.rubble.push(Rubble {
                at,
                r: 2.5,
                n: 2,
                kind: RubbleKind::Stone(block),
            });
        }
    }

    /// Returns the flag's seat in the middle of the pit's floor.
    fn arena(&mut self, centre: &Centre) -> Option<Seat> {
        let ground_min = centre.pit.iter().map(|&c| self.g(c)).min().unwrap_or(0);
        let floor_y = ground_min - self.rng.int(4, 5);
        let floor_noise = Noise2(self.rng.next_u64() as u32);
        let sand = self.mats.style.surface == super::style::Surface::Sand;
        let mut pit: Vec<[i32; 2]> = centre.pit.iter().copied().collect();
        pit.sort_unstable();
        for &c in &pit {
            let top = self.g(c);
            for y in floor_y + 1..=top + 1 {
                self.air(c, y);
            }
            let n = floor_noise.at(c[0] as f32 / 3.0, c[1] as f32 / 3.0);
            let floor = if sand {
                if n > 0.2 {
                    "gravel"
                } else {
                    "sand"
                }
            } else if n > 0.25 {
                "gravel"
            } else if n < -0.3 {
                "sand"
            } else {
                "coarse_dirt"
            };
            self.put_at(c, floor_y, &named(floor));
            self.ground.set(c, floor_y);
        }
        let mut rim: Vec<([i32; 2], [i32; 2])> = Vec::new();
        for &c in &pit {
            for d in Dir::ALL {
                let n = d.step(c, 1);
                if !centre.pit.contains(&n) && !rim.iter().any(|(r, _)| *r == n) {
                    rim.push((n, c));
                }
            }
        }
        let line_wood = !self.mats.stone && self.rng.roll(0.5);
        let wood = self.mats.wood;
        for &(r, _) in &rim {
            for y in floor_y + 1..=self.g(r) {
                if self.rng.roll(0.78) {
                    let m = if line_wood {
                        self.mats.log(wood, Axis::Y)
                    } else {
                        let s = self.mats.stone_at(&mut self.rng, [r[0], y, r[1]]);
                        self.mats.block(s)
                    };
                    self.put_at(r, y, &m);
                }
            }
        }
        let mut walked = std::collections::BTreeSet::new();
        for walk in &centre.walkways {
            let rim_c = [
                (centre.at[0] as f32 + walk.dir[0] * (walk.rim + 1.0)).round() as i32,
                (centre.at[1] as f32 + walk.dir[1] * (walk.rim + 1.0)).round() as i32,
            ];
            let far_c = [
                (centre.at[0] as f32 + walk.dir[0] * walk.far).round() as i32,
                (centre.at[1] as f32 + walk.dir[1] * walk.far).round() as i32,
            ];
            let rim_level = (self.g(rim_c) + 3) as f32;
            let outer = (self.g(far_c) + 1) as f32;
            let level_at = |along: f32| {
                if along <= walk.rim + 1.0 {
                    return rim_level;
                }
                let t = ((walk.far - along) / (walk.far - walk.rim - 1.0)).clamp(0.0, 1.0);
                ((outer + 0.5 + (rim_level - outer - 0.5) * t) * 2.0).round() / 2.0
            };
            for &(c, along) in &walk.cells {
                let level = level_at(along);
                let deck_y = if level.fract() == 0.0 {
                    level as i32 - 1
                } else {
                    level.floor() as i32
                };
                for y in (self.g(c) + 1).max(deck_y)..deck_y + 4 {
                    if self.plan.get([c[0], y, c[1]]).is_some() {
                        self.air(c, y);
                    }
                }
                let w = self.mats.wood_at(&mut self.rng, 0.05);
                let deck = if level.fract() == 0.0 {
                    self.mats.block(w)
                } else {
                    self.mats.slab(w, Half::Bottom)
                };
                self.put_at(c, deck_y, &deck);
                walked.insert(c);
                let in_pit = centre.pit.contains(&c);
                let post =
                    ((along * 2.0).round() as i32).rem_euclid(5) == 0 || along < walk.tip + 0.6;
                if post && deck_y - self.g(c) >= 2 {
                    for y in self.g(c) + 1..deck_y {
                        let m = if self.mats.stone && !in_pit {
                            let s = self.mats.stone_at(&mut self.rng, [c[0], y, c[1]]);
                            self.mats.block(s)
                        } else {
                            self.mats.fence(wood)
                        };
                        self.put_at(c, y, &m);
                    }
                }
                if in_pit && (along < walk.tip + 0.6 || self.rng.roll(0.4)) {
                    let m = self.mats.fence(wood);
                    self.put_at(c, deck_y + 1, &m);
                }
            }
        }
        for &(r, _) in &rim {
            if walked.contains(&r) || !self.rng.roll(0.62) {
                continue;
            }
            let y = self.g(r) + 1;
            if self.plan.get([r[0], y, r[1]]).is_some() {
                continue;
            }
            let m = if self.mats.stone {
                let s = self.mats.stone_at(&mut self.rng, [r[0], y, r[1]]);
                if self.rng.roll(0.7) {
                    self.mats.slab(s, Half::Bottom)
                } else {
                    self.mats.block(s)
                }
            } else {
                let w = self.mats.wood_at(&mut self.rng, 0.05);
                self.mats.fence(w)
            };
            self.put_at(r, y, &m);
        }
        let entries: Vec<([i32; 2], [i32; 2])> = rim
            .iter()
            .copied()
            .filter(|(r, _)| !walked.contains(r) && self.g(*r) > floor_y + 1)
            .collect();
        if !entries.is_empty() {
            let (r, p) = *self.rng.pick(&entries);
            for y in floor_y + 1..=self.g(r) {
                let m = if line_wood {
                    self.mats.log(wood, Axis::Y)
                } else {
                    let s = self.mats.stone_at(&mut self.rng, [r[0], y, r[1]]);
                    self.mats.block(s)
                };
                self.put_at(r, y, &m);
            }
            let top = self.g(r) + 1;
            self.plan.unset([r[0], top, r[1]]);
            let facing = Dir::of((p[0] - r[0]) as f32, (p[1] - r[1]) as f32);
            for y in floor_y + 1..=self.g(r) {
                self.put_at(p, y, &decor!("ladder").facing(facing));
            }
        }
        // The middle stays clear for the flag.
        let open: Vec<[i32; 2]> = pit
            .iter()
            .copied()
            .filter(|c| !walked.contains(c) && *c != centre.at)
            .collect();
        for _ in 0..self.rng.int(2, 4) {
            if open.is_empty() {
                break;
            }
            let c = *self.rng.pick(&open);
            if self
                .plan
                .get([c[0], floor_y + 1, c[1]])
                .is_some_and(|m| !m.is_air())
            {
                continue;
            }
            let w = self.mats.wood_at(&mut self.rng, 0.1);
            for y in floor_y + 1..=floor_y + self.rng.int(1, 2) {
                let m = self.mats.fence(w);
                self.put_at(c, y, &m);
            }
        }
        if self.rng.roll(0.4) && !open.is_empty() {
            let c = *self.rng.pick(&open);
            if self
                .plan
                .get([c[0], floor_y + 1, c[1]])
                .is_none_or(|m| m.is_air())
            {
                let facing = *self.rng.pick(&Dir::ALL);
                self.put_at(c, floor_y + 1, &named!("chest").facing(facing));
                loot(
                    &mut self.plan,
                    [c[0], floor_y + 1, c[1]],
                    crate::keys::CAMP_ARENA_LOOT,
                );
            }
        }
        self.layout.flagged.then_some(Seat::Ground(centre.at))
    }
}
