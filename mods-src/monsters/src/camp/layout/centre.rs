//! Where the centrepiece stands, and the arena pit with its walkways.

use std::f32::consts::TAU;

use mod_sdk::build::Draw;
use mod_sdk::{FxHashMap, FxHashSet, GenRng};

use super::*;
use crate::camp::{Camp, Res};

impl Camp<'_> {
    pub(super) fn lay_centre(&mut self) {
        let mut kind = *self.rng.weighted(&[
            (None, 15.0),
            (Some(CentreKind::Well), 30.0),
            (Some(CentreKind::Statue), 27.0),
            (Some(CentreKind::Arena), 28.0),
        ]);
        if kind == Some(CentreKind::Arena) && self.radius < 15.0 {
            kind = Some(*self.rng.pick(&[CentreKind::Well, CentreKind::Statue]));
        }
        let Some(kind) = kind else { return };
        let ra = (self.radius * 0.3).clamp(4.5, 7.5);
        let need = match kind {
            CentreKind::Well => 3.0,
            CentreKind::Statue => 4.0,
            CentreKind::Arena => ra + 2.0,
        };
        // Only the discs the tries below can test: jittered by up to 0.3 radius off the centre.
        let (field_lo, field_hi) = self.field.bounds();
        let span = (self.radius * 0.3).ceil() as i32 + (need + 0.5).ceil() as i32 + 1;
        let lo = [0, 1].map(|a| (self.center[a] - span).max(field_lo[a]));
        let hi = [0, 1].map(|a| (self.center[a] + span).min(field_hi[a]));
        let (x0, x1) = (
            (lo[0] - field_lo[0]) as usize,
            (hi[0] - field_lo[0]) as usize,
        );
        let depth_rows = self.field.depth_rows().skip((lo[1] - field_lo[1]) as usize);
        let room = RowCounts::from_rows(
            lo,
            hi,
            depth_rows.zip(lo[1]..=hi[1]).map(|(depths, z)| {
                let depths = &depths[x0..=x1];
                let resv = self.resv.row(z, lo[0], hi[0]);
                let plateau = self.plateau_at.row(z, lo[0], hi[0]);
                depths
                    .iter()
                    .zip(resv)
                    .zip(plateau)
                    .map(move |((&d, &r), &p)| {
                        d >= 4
                            && p == 0
                            && (r == Res::Free || (kind == CentreKind::Arena && r == Res::Bridge))
                    })
            }),
        );
        let rows = disc_rows(need + 0.5);
        for t in 0..60 {
            let jitter = |rng: &mut GenRng| if t == 0 { 0.0 } else { rng.range(-1.0, 1.0) };
            let at = [
                (self.center[0] as f32 + jitter(&mut self.rng) * self.radius * 0.3).round() as i32,
                (self.center[1] as f32 + jitter(&mut self.rng) * self.radius * 0.3).round() as i32,
            ];
            if !room.all_in_disc(at, &rows) {
                continue;
            }
            for c in disc(at, need + 0.5) {
                if self.resv.get(c) != Res::Bridge {
                    self.resv.set(c, Res::Centre);
                }
            }
            let mut centre = Centre {
                kind,
                at,
                need,
                ra,
                pit_shape: [(); 2].map(|()| {
                    let (amp, p) = (self.rng.range(0.0, 0.12), self.rng.range(0.0, TAU));
                    (amp, p.cos(), p.sin())
                }),
                pit: FxHashSet::default(),
                walkways: Vec::new(),
            };
            if kind == CentreKind::Arena {
                self.plan_arena(&mut centre);
            }
            self.centre = Some(centre);
            return;
        }
    }

    pub(super) fn plan_arena(&mut self, cp: &mut Centre) {
        let reach = cp.ra * 1.3;
        for c in disc(cp.at, reach) {
            let (dx, dz) = ((c[0] - cp.at[0]) as f32, (c[1] - cp.at[1]) as f32);
            let d = (dx * dx + dz * dz).sqrt();
            let (cos, sin) = if d > 0.0 {
                (dx / d, dz / d)
            } else {
                (1.0, 0.0)
            };
            if d <= cp.pit_radius_toward(cos, sin) {
                cp.pit.insert(c);
            }
        }
        let n = self.rng.int(2, 3);
        let a0 = self.rng.range(0.0, TAU);
        for k in 0..n {
            let a = a0 + k as f32 * TAU / n as f32 + self.rng.range(-0.3, 0.3);
            let (ux, uz) = (a.cos(), a.sin());
            let rim = cp.pit_radius(a);
            let tip = rim - 2.0;
            let mut len = self.rng.int(5, 8);
            while len >= 3 {
                let far = rim + 1.0 + len as f32;
                let mut cells: FxHashMap<[i32; 2], f32> = FxHashMap::default();
                let mut ok = true;
                let mut s = tip;
                'walk: while s <= far {
                    for off in [-0.5f32, 0.5] {
                        let c = [
                            (cp.at[0] as f32 + ux * s - uz * off).round() as i32,
                            (cp.at[1] as f32 + uz * s + ux * off).round() as i32,
                        ];
                        let r = self.resv.get(c);
                        if !self.ground.contains(c)
                            || (!cp.pit.contains(&c)
                                && (!self.inside(c)
                                    || self.plateau_at.get(c) > 0
                                    || self.depth(c) < 3
                                    || (r != Res::Free && r != Res::Centre)))
                        {
                            ok = false;
                            break 'walk;
                        }
                        let along = (c[0] - cp.at[0]) as f32 * ux + (c[1] - cp.at[1]) as f32 * uz;
                        let e = cells.entry(c).or_insert(along);
                        *e = e.min(along);
                    }
                    s += 0.35;
                }
                if ok {
                    let mut cells: Vec<([i32; 2], f32)> = cells.into_iter().collect();
                    cells.sort_by_key(|(c, _)| *c);
                    for (c, _) in &cells {
                        self.resv.set(*c, Res::Centre);
                    }
                    cp.walkways.push(Walkway {
                        dir: [ux, uz],
                        rim,
                        tip,
                        far,
                        cells,
                    });
                    break;
                }
                len -= 1;
            }
        }
    }
}
