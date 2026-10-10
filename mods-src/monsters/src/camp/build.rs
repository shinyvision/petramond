//! Building a laid-out camp into a plan. Each pass writes blocks; what a later pass needs from an
//! earlier one is handed to it ([`Fortress`], the tower tops, the flag's [`Seat`]), so the order
//! below is the order the passes' inputs allow.

use mod_sdk::build::{Material, Plan};
use mod_sdk::GenRng;

use super::flag::Seat;
use super::grid::Grid;
use super::ground::Rubble;
use super::layout::Layout;
use super::style::Mats;
use super::survey::Outline;
use super::Res;

/// What the fortress sections leave for the gates through them and the guards on them.
pub(super) struct Fortress {
    /// The fortress wall's height above its foot.
    pub h: i32,
    /// Per ring index, how far a collapsed section has fallen in.
    pub cut: Vec<i32>,
}

/// A camp built: its plan, and what the build settled that the layout did not.
pub(super) struct Built {
    pub plan: Plan,
    /// The floor as built: the arena's pit floor where one was dug.
    #[cfg(test)]
    pub ground: Grid<i32>,
    #[cfg(test)]
    pub posts: Vec<super::posts::Post>,
}

/// A camp being built from its layout, which the passes only read.
pub(super) struct Builder<'a> {
    pub rng: GenRng,
    pub mats: Mats<'a>,
    pub outline: &'a Outline,
    pub layout: &'a Layout,
    pub plan: Plan,
    pub ground: Grid<i32>,
    /// The columns a ladder or a stair run took during the build.
    access: Grid<bool>,
    /// Where something fell, strewn once everything stands.
    pub rubble: Vec<Rubble>,
}

impl<'a> Builder<'a> {
    pub(super) fn new(
        rng: GenRng,
        mats: Mats<'a>,
        outline: &'a Outline,
        layout: &'a Layout,
        ground: Grid<i32>,
    ) -> Builder<'a> {
        Builder {
            rng,
            mats,
            outline,
            layout,
            plan: Plan::new(),
            access: Grid::new(ground.min(), ground.size(), false),
            ground,
            rubble: Vec::new(),
        }
    }

    pub(super) fn build(mut self) -> Built {
        self.build_ground();
        self.build_walls();
        let fortress = self.build_fortress();
        self.build_gates(&fortress);
        let tower_tops = self.build_towers();
        let centre_seat = self.build_centre(&tower_tops);
        self.build_huts();
        self.build_access();
        self.build_bridges();
        self.scatter_rubble();
        self.wear_and_decorate();
        let ground = &self.ground;
        self.plan
            .settle_slabs(self.mats.families(), |x, z| ground.get([x, z]));
        self.snow();
        let seat = centre_seat.or(self.layout.flag_spot.map(Seat::Ground));
        if let Some(seat) = seat {
            self.raise_flag(seat);
        }
        // The posts are marked into the plan; the list itself only tells tests where.
        #[cfg_attr(not(test), allow(unused_variables))]
        let posts = self.build_posts(&fortress, &tower_tops);
        self.clear_space();
        Built {
            plan: self.plan,
            #[cfg(test)]
            ground: self.ground,
            #[cfg(test)]
            posts,
        }
    }

    pub(super) fn g(&self, c: [i32; 2]) -> i32 {
        self.ground.get(c)
    }

    /// What column `c` is reserved for: the layout's reservation, or [`Res::Access`] once a
    /// ladder or stair run took it.
    pub(super) fn resv(&self, c: [i32; 2]) -> Res {
        if self.access.get(c) {
            Res::Access
        } else {
            self.layout.resv.get(c)
        }
    }

    pub(super) fn take_access(&mut self, c: [i32; 2]) {
        self.access.set(c, true);
    }

    pub(super) fn put_at(&mut self, [x, z]: [i32; 2], y: i32, m: &Material) {
        self.plan.set([x, y, z], m);
    }

    pub(super) fn air(&mut self, [x, z]: [i32; 2], y: i32) {
        self.plan.set([x, y, z], &Material::air());
    }
}
