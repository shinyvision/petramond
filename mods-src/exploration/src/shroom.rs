use mod_sdk::GenRng;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Part {
    Stem,
    Cap,
    Gill,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Form {
    Flatcap,
    Round,
}

#[derive(Copy, Clone, Debug)]
pub struct Giant {
    pub form: Form,
    pub height: i32,
    pub stem_r: i32,
    pub cap_r: i32,
    pub skirt: i32,
    pub lean_x: i32,
    pub lean_z: i32,
}

impl Giant {
    pub fn roll(rng: &mut GenRng, scale: u8) -> Giant {
        let height = 5 + rng.next_i32(0, 4) + (scale as i32 * 4 / 255);
        let stem_r = height / 9;
        let (form, cap_r, skirt) = if rng.next_i32(0, 1) == 1 {
            (Form::Round, 4 + height / 6, (height / 8 - 1).max(0))
        } else {
            (Form::Flatcap, 3 + height / 3, 1)
        };
        Giant {
            form,
            height,
            stem_r,
            cap_r,
            skirt,
            lean_x: rng.next_i32(-4, 4),
            lean_z: rng.next_i32(-4, 4),
        }
    }

    pub fn reach(&self) -> i32 {
        self.cap_r + self.stem_r + (self.lean_x.abs().max(self.lean_z.abs()) * self.height) / 16 + 1
    }

    pub fn rise(&self) -> i32 {
        self.seat()
            + match self.form {
                Form::Flatcap => 0,
                Form::Round => self.cap_r,
            }
    }

    fn seat(&self) -> i32 {
        self.height - 1
    }

    pub fn cap_footprint(&self) -> (i32, i32, i32) {
        let (cx, cz) = self.axis(self.seat());
        (cx, cz, self.cap_r)
    }

    pub fn cap_levels(&self) -> (i32, i32) {
        let seat = self.seat();
        match self.form {
            Form::Flatcap => (seat - self.skirt, seat),
            Form::Round => (seat - self.skirt, seat + self.cap_r),
        }
    }

    /// Sparse skeleton for the all-or-nothing fit test, as offsets from the
    /// root: the leaning stem axis every second level, the cap rim at eight
    /// compass points, the plate interior (or mid-dome ring) at half radius,
    /// and the apex for a dome. ~20 cells. A body is several hundred cells, so
    /// this cannot see a one-cell rock needle threading between probes — but a
    /// wall or ceiling close enough to clip a mushroom crosses several of
    /// them, and that is the failure being screened for.
    pub fn fit_probes(&self, mut out: impl FnMut(i32, i32, i32)) {
        let seat = self.seat();
        let mut level = 1;
        while level < self.height {
            let (ax, az) = self.axis(level);
            out(ax, level, az);
            level += 2;
        }
        let (bx, bz) = self.axis(seat);
        let r = self.cap_r;
        let mut h = r;
        while 2 * h * h > r * r {
            h -= 1;
        }
        let ring = [
            (r, 0),
            (-r, 0),
            (0, r),
            (0, -r),
            (h, h),
            (h, -h),
            (-h, h),
            (-h, -h),
        ];
        let (rim_y, mid) = match self.form {
            Form::Flatcap => (seat, seat),
            Form::Round => (seat, seat + self.cap_r / 2),
        };
        for &(dx, dz) in &ring {
            out(bx + dx, rim_y, bz + dz);
        }
        for &(dx, dz) in &[
            (r / 2, r / 2),
            (r / 2, -r / 2),
            (-r / 2, r / 2),
            (-r / 2, -r / 2),
        ] {
            out(bx + dx, mid, bz + dz);
        }
        if let Form::Round = self.form {
            out(bx, seat + self.cap_r, bz);
        }
    }

    fn axis(&self, level: i32) -> (i32, i32) {
        ((self.lean_x * level) / 16, (self.lean_z * level) / 16)
    }

    /// Emit every cell of the mushroom as `(dx, dy, dz, part)` offsets from the
    /// root cell. Deterministic and allocation-free.
    pub fn emit(&self, mut out: impl FnMut(i32, i32, i32, Part)) {
        let seat = self.seat();
        // A round cap is HOLLOW, so the stem has to carry on up through the
        // void to the dome's inner apex; stop one short of the shell itself
        // and the cap hangs over the stalk joined to nothing.
        // A flatcap's stem stops one level BELOW the plate: the seat level is
        // the plate's, and a stem reaching it punches a stem-tile plus-shape
        // through the cap's top face (first write wins).
        let stem_levels = match self.form {
            Form::Flatcap => seat,
            Form::Round => self.height + self.cap_r - 2,
        };
        let mut prev = self.axis(0);
        for level in 0..stem_levels {
            let axis = self.axis(level.min(seat));
            // A leaning stem steps its axis between levels, and a step is a
            // DIAGONAL move for the cells: a thin stem would touch only at a
            // corner and come apart at every bend. Walk the step as an L inside
            // the level it happens on, so consecutive levels always share a
            // column.
            let mut waypoints = [axis; 3];
            let mut n = 1;
            if prev != axis {
                for w in [(axis.0, prev.1), prev] {
                    if !waypoints[..n].contains(&w) {
                        waypoints[n] = w;
                        n += 1;
                    }
                }
            }
            for &(ax, az) in &waypoints[..n] {
                for dz in -self.stem_r..=self.stem_r {
                    for dx in -self.stem_r..=self.stem_r {
                        if self.stem_r > 0 && dx.abs() == self.stem_r && dz.abs() == self.stem_r {
                            continue;
                        }
                        out(ax + dx, level, az + dz, Part::Stem);
                    }
                }
            }
            prev = axis;
        }

        let (bx, bz) = self.axis(seat);
        match self.form {
            Form::Flatcap => self.emit_flatcap(bx, seat, bz, &mut out),
            Form::Round => self.emit_round(bx, seat, bz, &mut out),
        }
    }

    fn emit_flatcap(&self, bx: i32, seat: i32, bz: i32, out: &mut impl FnMut(i32, i32, i32, Part)) {
        let r = self.cap_r;
        let r2 = r * r;
        let inner = (r - 2).max(self.stem_r + 1);
        let inner2 = inner * inner;
        for dz in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dz * dz;
                if d2 > r2 {
                    continue;
                }
                out(bx + dx, seat, bz + dz, Part::Cap);
                if d2 > inner2 {
                    out(bx + dx, seat - self.skirt, bz + dz, Part::Gill);
                }
            }
        }
    }

    fn emit_round(&self, bx: i32, seat: i32, bz: i32, out: &mut impl FnMut(i32, i32, i32, Part)) {
        let r = self.cap_r;
        let (outer2, inner2) = (r * r, (r - 2).max(0) * (r - 2).max(0));
        let shell = |d2: i32| d2 <= outer2 && d2 > inner2;
        for dy in 0..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    if shell(dx * dx + dy * dy + dz * dz) {
                        out(bx + dx, seat + dy, bz + dz, Part::Cap);
                    }
                }
            }
        }
        for k in 1..=self.skirt {
            for dz in -r..=r {
                for dx in -r..=r {
                    if shell(dx * dx + dz * dz) {
                        out(bx + dx, seat - k, bz + dz, Part::Gill);
                    }
                }
            }
        }
    }
}

pub fn vine_len(rng: &mut GenRng, max_len: i32) -> i32 {
    rng.next_i32(1, max_len.max(1))
}

pub fn vine_run(rng: &mut GenRng, max_len: i32, mut out: impl FnMut(i32)) {
    for d in 0..vine_len(rng, max_len) {
        out(-d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(g: &Giant) -> Vec<(i32, i32, i32, Part)> {
        let mut v = Vec::new();
        g.emit(|x, y, z, p| v.push((x, y, z, p)));
        v
    }

    #[test]
    fn emit_is_deterministic_for_equal_parameters() {
        for form in [Form::Flatcap, Form::Round] {
            let g = Giant {
                form,
                height: 11,
                stem_r: 1,
                cap_r: 5,
                skirt: 3,
                lean_x: 3,
                lean_z: -2,
            };
            assert_eq!(cells(&g), cells(&g));
        }
    }

    #[test]
    fn every_cell_is_within_the_advertised_reach_and_rise() {
        for form in [Form::Flatcap, Form::Round] {
            for height in 5..20 {
                for cap_r in 2..9 {
                    let g = Giant {
                        form,
                        height,
                        stem_r: 1,
                        cap_r,
                        skirt: 1 + cap_r / 2,
                        lean_x: 4,
                        lean_z: -4,
                    };
                    let (reach, rise) = (g.reach(), g.rise());
                    for (dx, dy, dz, _) in cells(&g) {
                        assert!(
                            dx.abs() <= reach && dz.abs() <= reach,
                            "cell ({dx},{dz}) escapes reach {reach} for {g:?}"
                        );
                        assert!(dy <= rise, "cell dy {dy} escapes rise {rise} for {g:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_flatcap_stem_stays_under_its_plate() {
        for height in 5..16 {
            for (lean_x, lean_z) in [(0, 0), (4, -4), (-3, 2)] {
                let g = Giant {
                    form: Form::Flatcap,
                    height,
                    stem_r: height / 9,
                    cap_r: 3 + height / 3,
                    skirt: 1,
                    lean_x,
                    lean_z,
                };
                let seat = g.height - 1;
                let mut cap_roofs_axis = false;
                g.emit(|dx, dy, dz, p| {
                    if p == Part::Stem {
                        assert!(
                            dy < seat,
                            "stem cell at level {dy} reaches the plate (seat {seat}) for {g:?}"
                        );
                    }
                    if dy == seat && p == Part::Cap && (dx, dz) == g.axis(seat) {
                        cap_roofs_axis = true;
                    }
                });
                assert!(
                    cap_roofs_axis,
                    "the plate does not roof the stem axis for {g:?}"
                );
            }
        }
    }

    #[test]
    fn fit_probes_are_a_subset_of_the_body() {
        for form in [Form::Flatcap, Form::Round] {
            for height in 5..16 {
                for cap_r in 3..8 {
                    for (lean_x, lean_z) in [(0, 0), (4, -4), (-3, 2)] {
                        let g = Giant {
                            form,
                            height,
                            stem_r: 1,
                            cap_r,
                            skirt: (height / 8 - 1).max(0),
                            lean_x,
                            lean_z,
                        };
                        let body: std::collections::BTreeSet<(i32, i32, i32)> = cells(&g)
                            .into_iter()
                            .map(|(x, y, z, _)| (x, y, z))
                            .collect();
                        g.fit_probes(|x, y, z| {
                            assert!(
                                body.contains(&(x, y, z)),
                                "fit probe ({x},{y},{z}) is not a body cell of {g:?}"
                            );
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn a_rolled_mushroom_is_one_connected_body() {
        for i in 0..600i32 {
            let mut rng = GenRng::positional(0xC0FFEE, 0x5EED, i, i * 7, i * 13);
            let scale = rng.next_i32(0, 255) as u8;
            let g = Giant::roll(&mut rng, scale);
            let cells: std::collections::HashSet<(i32, i32, i32)> = cells(&g)
                .into_iter()
                .map(|(x, y, z, _)| (x, y, z))
                .collect();
            let start = *cells.iter().min_by_key(|c| c.1).unwrap();
            let mut seen = std::collections::HashSet::new();
            let mut stack = vec![start];
            seen.insert(start);
            while let Some((x, y, z)) = stack.pop() {
                for (dx, dy, dz) in [
                    (1, 0, 0),
                    (-1, 0, 0),
                    (0, 1, 0),
                    (0, -1, 0),
                    (0, 0, 1),
                    (0, 0, -1),
                ] {
                    let n = (x + dx, y + dy, z + dz);
                    if cells.contains(&n) && seen.insert(n) {
                        stack.push(n);
                    }
                }
            }
            assert_eq!(
                seen.len(),
                cells.len(),
                "{} of {} cells are detached from the root for {g:?}",
                cells.len() - seen.len(),
                cells.len()
            );
        }
    }

    #[test]
    fn cap_is_wider_than_its_stem() {
        for form in [Form::Flatcap, Form::Round] {
            let g = Giant {
                form,
                height: 10,
                stem_r: 1,
                cap_r: 5,
                skirt: 3,
                lean_x: 0,
                lean_z: 0,
            };
            let c = cells(&g);
            let cap_w = c
                .iter()
                .filter(|(_, _, _, p)| *p != Part::Stem)
                .map(|(dx, _, _, _)| dx.abs())
                .max()
                .unwrap();
            let stem_w = c
                .iter()
                .filter(|(_, _, _, p)| *p == Part::Stem)
                .map(|(dx, _, _, _)| dx.abs())
                .max()
                .unwrap();
            assert!(cap_w > stem_w, "{form:?} cap {cap_w} vs stem {stem_w}");
        }
    }

    #[test]
    fn both_forms_roll_and_only_the_round_one_is_open_underneath() {
        let mut seen = [0usize; 2];
        for i in 0..600i32 {
            let mut rng = GenRng::positional(0xC0FFEE, 0x5EED, i, i * 7, i * 13);
            let scale = rng.next_i32(0, 255) as u8;
            let g = Giant::roll(&mut rng, scale);
            seen[usize::from(g.form == Form::Round)] += 1;

            let cap: Vec<(i32, i32, i32)> = cells(&g)
                .into_iter()
                .filter(|(_, _, _, p)| *p != Part::Stem)
                .map(|(x, y, z, _)| (x, y, z))
                .collect();
            let (bx, bz) = g.axis(g.seat());
            let filled = cap.contains(&(bx, g.seat(), bz));
            assert_eq!(
                filled,
                g.form == Form::Flatcap,
                "{:?}: cap fills its own axis at the seat = {filled} ({g:?})",
                g.form
            );
        }
        assert!(
            seen[0] > 0 && seen[1] > 0,
            "both forms must roll ({seen:?})"
        );
    }
}
