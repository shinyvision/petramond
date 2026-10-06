//! The camp's standard: a skull flag on a pole, flown when the flags pack is loaded. It stands in
//! the middle of an open camp, on the well's roof, on the arena's floor or up on a watch tower by
//! a statue, and the garrison only refills a camp that still flies one.

use mod_sdk::build::{Draw, Material};

use super::build::Builder;
use super::layout::{disc, Planner};
use super::Res;
use crate::keys::{FLAGPOLE, SKULL_FLAG};

/// How many cells the pole stands, the flag's own cell on top included.
const POLE_CELLS: (i32, i32) = (2, 5);

/// How far from the camp's centre the flag of a camp without a centrepiece may stand.
const GROUND_REACH: f32 = 4.0;

#[derive(Clone, Copy, Debug)]
pub(super) enum Seat {
    /// On the camp's ground in this column.
    Ground([i32; 2]),
    /// On the highest thing built in this column, between these heights.
    Top([i32; 2], i32, i32),
}

impl Planner<'_> {
    /// Keeps open ground near the middle of a camp without a centrepiece for the flag, before
    /// the paths and huts are laid.
    pub(super) fn lay_flag(&mut self) {
        if !self.layout.flagged || self.layout.centre.is_some() {
            return;
        }
        let center = self.outline.center;
        let mut spots: Vec<[i32; 2]> = disc(center, GROUND_REACH)
            .filter(|&c| {
                self.ground.contains(c)
                    && self.outline.depth(c) >= 3
                    && self.layout.plateau_at.get(c) == 0
                    && self.layout.resv.get(c) == Res::Free
            })
            .collect();
        let gap = |c: &[i32; 2]| (c[0] - center[0]).pow(2) + (c[1] - center[1]).pow(2);
        spots.sort_by_key(|c| (gap(c), *c));
        let Some(&at) = spots.first() else {
            return;
        };
        for c in disc(at, 1.5) {
            self.layout.reserve(c, Res::Centre);
        }
        self.layout.flag_spot = Some(at);
    }
}

impl Builder<'_> {
    /// A seat on the outer corner of one of the watch towers; `tower_tops` are their platform
    /// floors.
    pub(super) fn tower_seat(&mut self, tower_tops: &[i32]) -> Option<Seat> {
        let towers = &self.layout.towers;
        if towers.is_empty() {
            return None;
        }
        let t = self.rng.int(0, towers.len() as i32 - 1) as usize;
        let (s, min, top) = (towers[t].s, towers[t].min, tower_tops[t]);
        let corners = [0, s - 1].into_iter().flat_map(|dz| {
            [0, s - 1]
                .into_iter()
                .map(move |dx| [min[0] + dx, min[1] + dz])
        });
        let center = self.outline.center;
        let gap = |c: [i32; 2]| (c[0] - center[0]).pow(2) + (c[1] - center[1]).pow(2);
        let corner = corners.max_by_key(|&c| (gap(c), c))?;
        Some(Seat::Top(corner, self.g(corner) + 1, top + 8))
    }

    /// Stands the pole and its flag on `seat`.
    pub(super) fn raise_flag(&mut self, seat: Seat) {
        let (c, foot) = match seat {
            Seat::Ground(c) => (c, self.g(c) + 1),
            Seat::Top(c, lo, hi) => {
                let top = (lo..=hi).rev().find_map(|y| {
                    let m = self.plan.get([c[0], y, c[1]])?;
                    m.occupies().then_some((y, m.solid_top()))
                });
                match top {
                    // Something without a flat top (a bottom slab) gives way to the pole.
                    Some((y, flat)) => (c, if flat { y + 1 } else { y }),
                    None => (c, self.g(c) + 1),
                }
            }
        };
        let cells = self.rng.int(POLE_CELLS.0, POLE_CELLS.1);
        let pole = Material::named(FLAGPOLE);
        for y in foot..foot + cells - 1 {
            self.put_at(c, y, &pole);
        }
        self.put_at(c, foot + cells - 1, &Material::named(SKULL_FLAG));
    }
}
