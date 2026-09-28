//! Paths from the gates, and room for huts.

use mod_sdk::build::{Dir, Draw, Frame, Noise2};

use super::*;
use crate::camp::{Camp, Res};

/// [`Camp::hut_room`] bits.
const FOOT: u8 = 1;
const MARGIN: u8 = 2;

pub(super) struct HutRoom {
    min: [i32; 2],
    width: usize,
    mask: Vec<u8>,
}

impl Camp<'_> {
    fn mark_path(&mut self, c: [i32; 2]) {
        if self.paths.contains(c) {
            self.paths.set(c, true);
            self.path_bounds = Some(match self.path_bounds {
                None => (c, c),
                Some((lo, hi)) => (
                    [lo[0].min(c[0]), lo[1].min(c[1])],
                    [hi[0].max(c[0]), hi[1].max(c[1])],
                ),
            });
        }
    }

    pub(super) fn lay_paths(&mut self) {
        let noise = Noise2(self.rng.next_u64() as u32);
        let (target, stop) = match &self.centre {
            Some(c) => (c.at, c.need + 0.5),
            None => (self.center, 1.5),
        };
        for gi in 0..self.gates.len() {
            let (c, out) = (self.ring[self.gates[gi].i], self.gates[gi].out);
            let (mut px, mut pz) = (c[0] as f32 - out[0] * 1.5, c[1] as f32 - out[1] * 1.5);
            for step in 0..120 {
                let (dx, dz) = (target[0] as f32 - px, target[1] as f32 - pz);
                let d = dx.hypot(dz);
                if d < stop {
                    break;
                }
                let wob = noise.at(step as f32 / 6.0, gi as f32 * 7.3) * 0.7;
                px += dx / d + (-dz / d) * wob * 0.6;
                pz += dz / d + (dx / d) * wob * 0.6;
                for off in [[0, 0], [(-dz / d).round() as i32, (dx / d).round() as i32]] {
                    let p = [px.round() as i32 + off[0], pz.round() as i32 + off[1]];
                    if self.inside(p) {
                        self.mark_path(p);
                        self.reserve(p, Res::Path);
                    }
                }
            }
            let (mut qx, mut qz) = (c[0] as f32, c[1] as f32);
            for step in 0..self.rng.int(4, 8) {
                qx += out[0] + noise.at(step as f32 / 3.0, gi as f32 * 3.1 + 50.0) * 0.35;
                qz += out[1] + noise.at(step as f32 / 3.0, gi as f32 * 3.1 + 80.0) * 0.35;
                for off in [[0, 0], [(-out[1]).round() as i32, out[0].round() as i32]] {
                    let p = [qx.round() as i32 + off[0], qz.round() as i32 + off[1]];
                    if self.ground.contains(p) && self.plateau_at.get(p) == 0 {
                        self.mark_path(p);
                    }
                }
            }
            for k in 0..self.gates[gi].idx.len() {
                let j = self.gates[gi].idx[k];
                self.mark_path(self.ring[j]);
            }
        }
    }

    /// Room for huts on open ground: per column inside the wall and off the plateaus, whether it
    /// is free ([`FOOT`]) and whether it is free or a path ([`MARGIN`], the ring a hut keeps
    /// around it).
    fn hut_room(&self) -> HutRoom {
        let (min, max) = self.field.bounds();
        let width = (max[0] - min[0] + 1) as usize;
        let mut mask = Vec::with_capacity(width * (max[1] - min[1] + 1) as usize);
        for (depths, z) in self.field.depth_rows().zip(min[1]..) {
            let resv = self.resv.row(z, min[0], max[0]);
            let plateau = self.plateau_at.row(z, min[0], max[0]);
            for ((&r, &p), &d) in resv.iter().zip(plateau).zip(depths) {
                mask.push(if p != 0 || d <= 0 {
                    0
                } else if r == Res::Free {
                    FOOT | MARGIN
                } else if r == Res::Path {
                    MARGIN
                } else {
                    0
                });
            }
        }
        HutRoom { min, width, mask }
    }

    /// [`hut_fits`](Camp::hut_fits) on open ground, from `room`.
    fn hut_fits_open(&self, room: &HutRoom, min: [i32; 2], size: [i32; 2], max_slope: i32) -> bool {
        let (lo_c, hi_c) = (
            [min[0] - 1, min[1] - 1],
            [min[0] + size[0], min[1] + size[1]],
        );
        let depth = room.mask.len() / room.width;
        let off = [lo_c[0] - room.min[0], lo_c[1] - room.min[1]];
        if off[0] < 0 || off[1] < 0 || (hi_c[0] - room.min[0]) as usize >= room.width {
            return false;
        }
        if (hi_c[1] - room.min[1]) as usize >= depth {
            return false;
        }
        let w = (size[0] + 2) as usize;
        for (k, z) in (lo_c[1]..=hi_c[1]).enumerate() {
            let at = (off[1] as usize + k) * room.width + off[0] as usize;
            let row = &room.mask[at..at + w];
            let edge = z == lo_c[1] || z == hi_c[1];
            let fits = if edge {
                row.iter().all(|&m| m & MARGIN != 0)
            } else {
                row[0] & MARGIN != 0
                    && row[w - 1] & MARGIN != 0
                    && row[1..w - 1].iter().all(|&m| m & FOOT != 0)
            };
            if !fits {
                return false;
            }
        }
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for z in min[1]..hi_c[1] {
            for &g in self.ground.row(z, min[0], hi_c[0] - 1) {
                lo = lo.min(g);
                hi = hi.max(g);
            }
        }
        hi - lo <= max_slope
    }

    /// Whether a hut's world footprint `min`, `size` and the ring of columns around it are free,
    /// inside the wall, on the right ground and no steeper than `max_slope`.
    pub(super) fn hut_fits(
        &self,
        min: [i32; 2],
        size: [i32; 2],
        plateau: Option<u8>,
        max_slope: i32,
    ) -> bool {
        let (lo_c, hi_c) = (
            [min[0] - 1, min[1] - 1],
            [min[0] + size[0], min[1] + size[1]],
        );
        if !self.ground.contains(lo_c) || !self.ground.contains(hi_c) {
            return false;
        }
        let top = plateau.map(|id| self.plateaus[id as usize - 1].top);
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for z in lo_c[1]..=hi_c[1] {
            let ground = self.ground.row(z, lo_c[0], hi_c[0]);
            let resv = self.resv.row(z, lo_c[0], hi_c[0]);
            let plateau_at = self.plateau_at.row(z, lo_c[0], hi_c[0]);
            let edge_row = z == lo_c[1] || z == hi_c[1];
            for (i, x) in (lo_c[0]..=hi_c[0]).enumerate() {
                if !self.inside([x, z]) {
                    return false;
                }
                let foot = !edge_row && x != lo_c[0] && x != hi_c[0];
                let r = resv[i];
                if r != Res::Free && (foot || r != Res::Path) {
                    return false;
                }
                let pid = plateau_at[i];
                match (plateau, top) {
                    (Some(id), Some(top)) if pid != id || ground[i] != top => return false,
                    (None, _) if pid > 0 => return false,
                    _ => {}
                }
                if foot {
                    lo = lo.min(ground[i]);
                    hi = hi.max(ground[i]);
                }
            }
        }
        hi - lo <= max_slope
    }

    pub(super) fn try_hut(
        &mut self,
        kind: HutKind,
        c: [i32; 2],
        plateau: Option<u8>,
        room: Option<&mut HutRoom>,
    ) -> bool {
        let dir = if self.rng.roll(0.75) {
            Dir::of(
                (self.center[0] - c[0]) as f32,
                (self.center[1] - c[1]) as f32,
            )
        } else {
            *self.rng.pick(&Dir::ALL)
        };
        let sizes: [Option<(i32, i32)>; 3] = match kind {
            HutKind::Hut => [
                Some((self.rng.int(4, 7), self.rng.int(4, 6))),
                Some((self.rng.int(4, 5), 4)),
                Some((4, 3)),
            ],
            HutKind::LeanTo => [
                Some((self.rng.int(3, 5), self.rng.int(3, 4))),
                Some((3, 3)),
                None,
            ],
            HutKind::AFrame => [
                Some((*self.rng.pick(&[3, 5]), self.rng.int(3, 6))),
                Some((3, 3)),
                None,
            ],
        };
        for (w, d) in sizes.into_iter().flatten() {
            let size = Frame::world_size(w, d, dir);
            let min = [c[0] - size[0] / 2, c[1] - size[1] / 2];
            let slope = if kind == HutKind::Hut { 3 } else { 2 };
            let fits = match (&room, plateau) {
                (Some(room), None) => self.hut_fits_open(room, min, size, slope),
                _ => self.hut_fits(min, size, plateau, slope),
            };
            if !fits {
                continue;
            }
            let frame = Frame::new(min, w, d, dir);
            for lz in -1..=d {
                for lx in -1..=w {
                    self.reserve(frame.at(lx, lz), Res::Hut);
                }
            }
            if let Some(room) = room {
                for z in min[1] - 1..=min[1] + size[1] {
                    for x in min[0] - 1..=min[0] + size[0] {
                        let at =
                            (z - room.min[1]) as usize * room.width + (x - room.min[0]) as usize;
                        if self.resv.get([x, z]) == Res::Hut {
                            room.mask[at] = 0;
                        }
                    }
                }
            }
            self.huts.push(Hut { kind, frame, w, d });
            return true;
        }
        false
    }

    pub(super) fn lay_huts(&mut self) {
        let area = self.interior.len() as f32;
        let target = (area / self.rng.range(110.0, 170.0))
            .round()
            .clamp(2.0, 8.0) as usize;
        let (min, max) = self.field.bounds();
        let mut cells: Vec<[i32; 2]> = Vec::with_capacity(self.interior.len());
        for (depths, z) in self.field.depth_rows().zip(min[1]..) {
            let resv = self.resv.row(z, min[0], max[0]);
            let plateau = self.plateau_at.row(z, min[0], max[0]);
            for (((&d, &r), &p), x) in depths.iter().zip(resv).zip(plateau).zip(min[0]..) {
                if d > 0 && r == Res::Free && p == 0 {
                    cells.push([x, z]);
                }
            }
        }
        self.rng.shuffle(&mut cells);
        let mut room = self.hut_room();
        for c in cells {
            if self.huts.len() >= target {
                break;
            }
            if self.resv.get(c) != Res::Free {
                continue;
            }
            let kind = *self.rng.weighted(&[
                (HutKind::Hut, 6.0),
                (HutKind::LeanTo, 2.5),
                (HutKind::AFrame, 1.5),
            ]);
            if !self.try_hut(kind, c, None, Some(&mut room)) && kind != HutKind::Hut {
                self.try_hut(HutKind::Hut, c, None, Some(&mut room));
            }
        }
        for p in 0..self.plateaus.len() {
            if !self.rng.roll(0.55) {
                continue;
            }
            let id = self.plateaus[p].id;
            let mut tops: Vec<[i32; 2]> = self.plateaus[p]
                .cells
                .iter()
                .copied()
                .filter(|&c| {
                    self.is_plateau_top(c) && self.inside(c) && self.resv.get(c) == Res::Free
                })
                .collect();
            self.rng.shuffle(&mut tops);
            for c in tops.into_iter().take(40) {
                let kind = *self.rng.weighted(&[
                    (HutKind::LeanTo, 2.0),
                    (HutKind::Hut, 1.0),
                    (HutKind::AFrame, 1.0),
                ]);
                if self.try_hut(kind, c, Some(id), None) {
                    break;
                }
            }
        }
    }
}
