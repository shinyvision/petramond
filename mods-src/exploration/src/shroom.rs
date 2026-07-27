//! Shape generators for the cavern's flora.
//!
//! Every generator is a PURE function of `(origin, rng seed)` and emits cell
//! offsets through a callback. Nothing here touches the world: the caller
//! decides which cells survive. That is what makes a mushroom that straddles a
//! section boundary come out seamless — each section re-derives the identical
//! shape and keeps only the cells it owns.

use mod_sdk::GenRng;

/// What a generated cell is, so the caller can map roles onto its own block ids
/// (and pick a species colour) without this module knowing any block names.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Part {
    Stem,
    Cap,
    /// Cap underside — the caller may want a dimmer or differently-tinted block
    /// so the gills read as shadowed from below.
    Gill,
}

/// Which cap a giant grew. Rolled per mushroom, and a genuinely different
/// generator either way rather than one profile with a knob on it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Form {
    /// A broad FLAT plate on a stem, its rim drooping one level so the
    /// silhouette is a parasol rather than a table top.
    Flatcap,
    /// A hollow dome whose wall carries straight on DOWN past the seat and
    /// stands around the stem — an inverted U in cross-section, open
    /// underneath, so the player can walk in under the cap.
    Round,
}

/// A giant mushroom's parameters, rolled once per origin.
#[derive(Copy, Clone, Debug)]
pub struct Giant {
    pub form: Form,
    /// Stem levels below the cap's seat.
    pub height: i32,
    pub stem_r: i32,
    pub cap_r: i32,
    /// Levels the cap's edge hangs BELOW its seat: one lip for a flatcap, a
    /// whole skirt for a round one.
    pub skirt: i32,
    /// Lean in 1/16 blocks per level, applied to the stem and carried by the cap
    /// so a leaning mushroom stays attached to its own stem.
    pub lean_x: i32,
    pub lean_z: i32,
}

impl Giant {
    /// Roll a mushroom. `scale` in 0..=255 biases toward the cathedral-scale end
    /// so a cavern can host a few landmark specimens among smaller ones.
    pub fn roll(rng: &mut GenRng, scale: u8) -> Giant {
        // Height drives everything else. The proportions matter more than the
        // absolute size: a tall mushroom needs a THICKER stem and a broader cap
        // or the silhouette reads as a lamppost with a table on top.
        // Caps MUST stay small enough not to fuse. At cap_r 8 (17 wide) and a
        // few per cavern they merge into solid pink walls — measured, it turns
        // the room into a corridor of mushroom flesh.
        let height = 5 + rng.next_i32(0, 4) + (scale as i32 * 4 / 255);
        let stem_r = height / 9;
        // A round cap is DEEP as well as wide, so it has to stay narrower than
        // a flat one to keep the same visual span and the same reach bound.
        // The round skirt is deliberately short (Rachel: ~3 blocks shorter than
        // the first tune): the dome itself already carries the inverted-U read,
        // and long skirt rings closed it into a bell.
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
            // Leaning reads as organic, but only gently: a hard lean detaches the
            // cap visually from the stem at these voxel scales.
            lean_x: rng.next_i32(-4, 4),
            lean_z: rng.next_i32(-4, 4),
        }
    }

    /// Horizontal reach in blocks — the margin a section must iterate with so a
    /// mushroom rooted in a neighbouring column is still re-derived here.
    pub fn reach(&self) -> i32 {
        self.cap_r + self.stem_r + (self.lean_x.abs().max(self.lean_z.abs()) * self.height) / 16 + 1
    }

    /// Levels the highest cell sits above the root — the vertical twin of
    /// [`reach`](Self::reach), and the bound the anchor scan's rise margin
    /// must cover.
    pub fn rise(&self) -> i32 {
        self.seat()
            + match self.form {
                Form::Flatcap => 0,
                Form::Round => self.cap_r,
            }
    }

    /// The level the cap seats on: the stem's top.
    fn seat(&self) -> i32 {
        self.height - 1
    }

    /// The cap's centre offset from the root column and its radius — the
    /// footprint two mushrooms actually fight over. Stems are one to three
    /// cells wide and essentially never collide; caps are what interpenetrate.
    pub fn cap_footprint(&self) -> (i32, i32, i32) {
        let (cx, cz) = self.axis(self.seat());
        (cx, cz, self.cap_r)
    }

    /// The cap's vertical span as levels above the root, lowest first.
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
        // Largest h with 2h² <= r², so the diagonal rim probes stay INSIDE the
        // cap: a probe on a cell the body does not occupy would veto the
        // mushroom for rock beside it, not rock through it.
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

    /// Stem-axis offset at a given level, from the lean.
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
        // --- stem -------------------------------------------------------
        let mut prev = self.axis(0);
        for level in 0..stem_levels {
            // Above the seat the stem is inside the cap, so it stops leaning:
            // the cap is anchored at the seat's axis and a stem that kept
            // drifting would walk out through the shell.
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
                        // clip the corners of a 3x3 stem so it reads round
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

    /// A flat plate seated on the stem, plus a lip hanging one level under its
    /// rim. The lip is the whole trick: a bare disc reads as a table top from
    /// inside the cavern, and two cells of droop turn it into a parasol.
    fn emit_flatcap(&self, bx: i32, seat: i32, bz: i32, out: &mut impl FnMut(i32, i32, i32, Part)) {
        let r = self.cap_r;
        let r2 = r * r;
        // The lip never reaches in as far as the stem, so the two never fight
        // over a column and the plate above always roofs it.
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

    /// A hollow hemispherical shell over the seat, carried on downward as a
    /// straight ring for `skirt` levels. The shell is TWO cells thick in the
    /// radial sense (`d² > (r-2)²`); a one-cell digital sphere leaves its apex
    /// touching the rest of the body only diagonally, which reads as detached
    /// blocks in the sky.
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
        // The skirt is the same ring the shell has at its equator, repeated
        // downward — so the join is exact and needs no profile matching. It is
        // open underneath by construction: nothing ever closes the bottom.
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

/// How far a curtain rooted on this stream reaches, as the FIRST draw after the
/// root's own gather roll. Exposed separately so a section can ask "can this run
/// reach me at all" before paying for the terrain probes that would answer it
/// properly — a run rooted above a section's roof usually stops short of it.
pub fn vine_len(rng: &mut GenRng, max_len: i32) -> i32 {
    rng.next_i32(1, max_len.max(1))
}

/// A hanging vine curtain dropped from a ceiling cell: a run of vine cells of
/// rolled length. Pure in `(origin, rng)`.
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

    /// The seam contract in miniature: the shape is a pure function of its
    /// parameters, so two independent derivations must agree cell for cell. If
    /// this ever fails, mushrooms straddling a section boundary will be sliced.
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

    /// Every emitted cell must sit inside the advertised horizontal reach, or a
    /// neighbouring section will not iterate far enough to re-derive it and the
    /// mushroom gets cut off at the border.
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

    /// The plate must ROOF the stem. A flatcap stem reaching the seat level
    /// punches a plus-shape of stem tiles through the cap's top face — first
    /// write wins — which is exactly the screenshot that reported it.
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

    /// Every fit probe must be a cell the mushroom itself occupies. A probe
    /// beside the body would veto the mushroom for rock NEXT TO it rather than
    /// rock THROUGH it, silently thinning the population near every wall —
    /// exactly the kind of drift a future shape edit would cause.
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

    /// A mushroom must be ONE 6-connected body. The cap is a hollow shell, and
    /// a shell whose rim is thinner than the taper leaves its crown resting on
    /// air — the cap silently becomes a lid floating over a ring. That reads as
    /// a worldgen seam bug (detached blocks in the sky) when it is really the
    /// cap profile, so it is pinned here across the whole rolled parameter
    /// space rather than left to a screenshot.
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

    /// The cap must actually overhang the stem, otherwise it reads as a pillar.
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

    /// Both forms have to be ROLLED, or a "variation" pass ships one shape.
    /// Also the shape contract that distinguishes them, checked on the rolls
    /// themselves: a round cap is open underneath (its widest level has a
    /// hollow middle) and a flatcap is not (its plate is solid).
    #[test]
    fn both_forms_roll_and_only_the_round_one_is_open_underneath() {
        let mut seen = [0usize; 2];
        for i in 0..600i32 {
            let mut rng = GenRng::positional(0xC0FFEE, 0x5EED, i, i * 7, i * 13);
            let scale = rng.next_i32(0, 255) as u8;
            let g = Giant::roll(&mut rng, scale);
            seen[usize::from(g.form == Form::Round)] += 1;

            // At the seat a flat plate is SOLID across the axis; a round cap is
            // already a ring there, which is what "open underneath" means.
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
            seen[0] > 200 && seen[1] > 200,
            "both forms must be common ({seen:?})"
        );
    }
}
