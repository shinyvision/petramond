use mod_sdk::build::{Dir, Draw, Form, Half, Material, Noise2};
use mod_sdk::{ColumnBox, ColumnMask, GenRng};

use super::build::Builder;
use super::layout::disc_rows;
use super::style::{decor, named, BiomeStyle, Surface};
use super::Res;

/// The highest a camp builds above its ground.
const BUILD_HEIGHT: i32 = 32;
/// Outside the wall, a band this wide is claimed so no tree grows against it.
pub(super) const CLEAR_MARGIN: f32 = 5.0;
/// How far above the natural ground its plants reach.
const PLANT_HEIGHT: i32 = 2;

/// `Layout::zone_class` bits: the column is the camp's own (wall line, interior or plateau).
pub(super) const IN_ZONE: u8 = 1;
/// Strictly inside the wall.
pub(super) const INSIDE: u8 = 2;
/// On a plateau.
pub(super) const PLATEAU: u8 = 4;
/// On a path from a gate.
pub(super) const ON_PATH: u8 = 8;

/// The block the style's natural ground already has on top, where the camp may leave ground it
/// did not reshape as it is; `None` where its own ground differs (podzol, snow cover).
pub(super) fn natural_top(style: &BiomeStyle) -> Option<Material> {
    match style.surface {
        _ if style.snow => None,
        Surface::Grass => Some(named!("grass")),
        Surface::Sand => Some(named!("sand")),
        Surface::Podzol => None,
    }
}

/// The camp's bare ground at a column, before wear.
fn surface_at(style: &BiomeStyle, noise: Noise2, [x, z]: [i32; 2]) -> Material {
    let n = |s: f32| noise.at(x as f32 / s, z as f32 / s);
    match style.surface {
        Surface::Sand => named!("sand"),
        Surface::Podzol if n(6.0) > -0.15 => named!("podzol"),
        _ if style.rocky && n(7.0) > 0.42 => {
            if noise.at(x as f32 / 2.0 + 90.0, z as f32 / 2.0) > 0.0 {
                named!("stone")
            } else {
                named!("gravel")
            }
        }
        _ if style.mud && n(5.0) > 0.45 => named!("mud"),
        _ => named!("grass"),
    }
}

pub(super) struct Rubble {
    pub at: [i32; 2],
    pub r: f32,
    pub n: i32,
    pub kind: RubbleKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RubbleKind {
    Camp,
    Wood,
    Stone(&'static str),
}

impl Builder<'_> {
    /// Levels the camp floor, raises the plateaus out of rock and lays the surface.
    pub(super) fn build_ground(&mut self) {
        let surface = Noise2(self.rng.next_u64() as u32);
        let rock = Noise2(self.rng.next_u64() as u32);
        let style = self.mats.style;
        let sand = style.surface == Surface::Sand;
        let [stone, sandstone, tuff, gravel, sand_m, dirt] =
            ["stone", "sandstone", "tuff", "gravel", "sand", "dirt"].map(named);
        let (lo, hi) = self.layout.zone;
        let width = (hi[0] - lo[0] + 1) as usize;
        let (natural, zone_class) = (&self.outline.natural, &self.layout.zone_class);
        let Builder { ground, plan, .. } = self;
        for (row, z) in zone_class.chunks(width).zip(lo[1]..) {
            let floor = ground.row(z, lo[0], hi[0]);
            let heights = natural.row(z, lo[0], hi[0]).unwrap_or(floor);
            for (((&class, &g), &n), x) in row.iter().zip(floor).zip(heights).zip(lo[0]..) {
                if class & IN_ZONE == 0 {
                    continue;
                }
                let plateau = class & PLATEAU != 0;
                for y in (n + 1).min(g)..g {
                    let m = if plateau && g - y >= 2 {
                        // Sheared with height, like `Mats::stone_at`: strata at a 2D sample's cost.
                        let v = rock.at(
                            x as f32 / 4.0 + y as f32 * 0.43,
                            z as f32 / 4.0 - y as f32 * 0.37,
                        );
                        if sand {
                            if v > 0.45 {
                                stone
                            } else {
                                sandstone
                            }
                        } else if v > 0.5 {
                            tuff
                        } else if v < -0.62 {
                            gravel
                        } else {
                            stone
                        }
                    } else if sand {
                        sand_m
                    } else {
                        dirt
                    };
                    plan.set([x, y, z], &m);
                }
                let top = surface_at(style, surface, [x, z]);
                if n != g || Some(top) != natural_top(style) {
                    plan.set([x, g, z], &top);
                }
            }
        }
    }

    /// Worn ground inside the wall and the paths from the gates, in noise-coherent patches so
    /// it reads as trampled earth rather than speckle, then tufts, dead bushes and pebbles on the
    /// open ground inside.
    pub(super) fn wear_and_decorate(&mut self) {
        let noise = Noise2(self.rng.next_u64() as u32);
        let style = self.mats.style;
        let sand = style.surface == Surface::Sand;
        let worn = ["grass", "podzol", "sand", "mud"].map(|s| named(s).block);
        let [grass, podzol] = ["grass", "podzol"].map(|s| named(s).block);
        let (coarse, gravel, dirt) = (named!("coarse_dirt"), named!("gravel"), named!("dirt"));
        let [short_grass, dead_bush] = ["short_grass", "dead_bush"].map(decor);
        let pebbles = ["pebbles_small", "pebbles_medium", "pebbles_large"].map(decor);
        let bare = natural_top(style).map(|m| m.block);
        let (lo, hi) = self.layout.zone;
        let width = (hi[0] - lo[0] + 1) as usize;
        let zone_class = &self.layout.zone_class;
        let Builder {
            ground, plan, rng, ..
        } = self;
        for (row, z) in zone_class.chunks(width).zip(lo[1]..) {
            let floor = ground.row(z, lo[0], hi[0]);
            for ((&class, &y), x) in row.iter().zip(floor).zip(lo[0]..) {
                let on_path = class & ON_PATH != 0;
                if !on_path && class & INSIDE == 0 {
                    continue;
                }
                let mut top = plan.get([x, y, z]).map(|m| m.block).or(bare);
                if (on_path || class & PLATEAU == 0) && top.is_none_or(|b| worn.contains(&b)) {
                    let n = noise.at(x as f32 / 3.2, z as f32 / 3.2);
                    let mut wear = if on_path {
                        *rng.weighted(&[
                            (Some(coarse), 40.0),
                            (Some(gravel), 18.0),
                            (Some(dirt), 26.0),
                            (None, 12.0),
                        ])
                    } else if n > 0.32 {
                        Some(coarse)
                    } else if n > 0.22 && rng.roll(0.7) {
                        Some(dirt)
                    } else if rng.roll(0.015) {
                        Some(gravel)
                    } else {
                        None
                    };
                    if sand && wear.is_some() {
                        wear =
                            (wear == Some(gravel) || (on_path && rng.roll(0.3))).then_some(gravel);
                    }
                    if let Some(m) = wear {
                        plan.set([x, y, z], &m);
                        top = Some(m.block);
                    }
                }
                let Some(top) = top else {
                    continue;
                };
                if on_path || plan.get([x, y + 1, z]).is_some() {
                    continue;
                }
                let r = rng.unit();
                let m = if (top == grass || top == podzol) && r < style.tufts * 0.35 {
                    short_grass
                } else if r > 1.0 - style.dead_bush * 2.0 {
                    dead_bush
                } else if r > 0.5 && r < 0.512 {
                    *rng.pick(&pebbles)
                } else {
                    continue;
                };
                plan.set([x, y + 1, z], &m);
            }
        }
    }

    /// The highest block the plan puts in a column, falling back to the ground.
    pub(super) fn top_of(&self, c: [i32; 2]) -> (i32, Option<Material>) {
        let g = self.g(c);
        for y in (g - 8..=g + BUILD_HEIGHT).rev() {
            if let Some(m) = self.plan.get([c[0], y, c[1]]) {
                if !m.is_air() {
                    return (y, Some(*m));
                }
            }
        }
        (g, None)
    }

    fn rubble_block(&mut self, kind: RubbleKind) -> Material {
        let r = self.rng.unit();
        match kind {
            RubbleKind::Stone(block) => named(block),
            RubbleKind::Wood => self.wood_rubble(r),
            RubbleKind::Camp if !self.mats.stone => self.wood_rubble(r),
            RubbleKind::Camp => {
                let s = *self
                    .rng
                    .pick(&["cobblestone", "cobblestone", "stone", "stone_bricks"]);
                if r < 0.35 {
                    self.mats.slab(s, Half::Bottom)
                } else if r < 0.55 {
                    let d = *self.rng.pick(&Dir::ALL);
                    self.mats.stairs(s, d, Half::Bottom)
                } else if r < 0.72 {
                    self.mats.block(s)
                } else if r < 0.82 {
                    named!("gravel")
                } else {
                    decor(
                        self.rng
                            .pick(&["pebbles_small", "pebbles_medium", "pebbles_large"]),
                    )
                }
            }
        }
    }

    fn wood_rubble(&mut self, r: f32) -> Material {
        let w = self.mats.wood_at(&mut self.rng, 0.1);
        if r < 0.4 {
            self.mats.slab(w, Half::Bottom)
        } else if r < 0.65 {
            let d = *self.rng.pick(&Dir::ALL);
            self.mats.stairs(w, d, Half::Bottom)
        } else if r < 0.85 {
            let axis = *self
                .rng
                .pick(&[mod_sdk::build::Axis::X, mod_sdk::build::Axis::Z]);
            self.mats.log(w, axis)
        } else {
            decor(self.rng.pick(&["pebbles_small", "pebbles_medium"]))
        }
    }

    pub(super) fn scatter_rubble(&mut self) {
        let spots = std::mem::take(&mut self.rubble);
        for spot in &spots {
            for _ in 0..spot.n {
                let c = [
                    (spot.at[0] as f32 + self.rng.range(-spot.r, spot.r)).round() as i32,
                    (spot.at[1] as f32 + self.rng.range(-spot.r, spot.r)).round() as i32,
                ];
                if !self.ground.contains(c) || self.layout.paths.get(c) {
                    continue;
                }
                if matches!(
                    self.resv(c),
                    Res::Gate | Res::Hut | Res::Access | Res::Centre
                ) {
                    continue;
                }
                let (y, under) = self.top_of(c);
                let solid = under
                    .as_ref()
                    .is_none_or(|m| matches!(m.form, Form::Block | Form::Log | Form::Other));
                if !solid
                    || self
                        .plan
                        .get([c[0], y + 1, c[1]])
                        .is_some_and(|m| !m.is_air())
                {
                    continue;
                }
                let m = self.rubble_block(spot.kind);
                self.put_at(c, y + 1, &m);
            }
        }
    }

    pub(super) fn snow(&mut self) {
        if !self.mats.style.snow {
            return;
        }
        let mut rng = GenRng::positional(
            self.outline.center[0] as u32,
            0x5e0,
            self.outline.center[0],
            0,
            self.outline.center[1],
        );
        let paths = &self.layout.paths;
        self.plan.cover(&decor!("snow_layer"), &mut rng, |x, z| {
            if paths.get([x, z]) {
                0.3
            } else {
                0.92
            }
        });
    }

    /// Claims the camp's columns (its zone, its paths and a band outside the wall) so no tree
    /// grows in them, and clears the natural ground and plants above the zone's floor.
    pub(super) fn clear_space(&mut self) {
        let (lo, hi) = self.layout.zone;
        let width = (hi[0] - lo[0] + 1) as usize;
        let mut claim = ColumnMask::empty(ColumnBox { min: lo, max: hi });
        // Every path from outside into the ring's area crosses the ring, so the band within
        // CLEAR_MARGIN of that area is the band within it of the ring itself. Ring cells are face
        // neighbours, so every other one with a disc a column wider covers them all.
        let spans = disc_rows(CLEAR_MARGIN + 1.0);
        for &r in self.outline.ring.iter().step_by(2) {
            for &(dz, half) in &spans {
                claim.insert_run(r[1] + dz, r[0] - half, r[0] + half);
            }
        }
        // One height above the floor everywhere: the floor is level over long runs, so the
        // cleared cells merge into a few boxes. Ranges start at the floor and are raised by that
        // height once it is known.
        let mut above = PLANT_HEIGHT;
        let mut ranges = Vec::with_capacity(self.layout.zone_class.len());
        for (row, z) in self.layout.zone_class.chunks(width).zip(lo[1]..) {
            let ground = self.ground.row(z, lo[0], hi[0]);
            let natural = self.outline.natural.row(z, lo[0], hi[0]).unwrap_or(ground);
            let mut run = None;
            for (((&class, &g), &n), x) in row.iter().zip(ground).zip(natural).zip(lo[0]..) {
                match (class & (IN_ZONE | ON_PATH) != 0, run) {
                    (true, None) => run = Some(x),
                    (false, Some(from)) => {
                        claim.insert_run(z, from, x - 1);
                        run = None;
                    }
                    _ => {}
                }
                ranges.push(if class & IN_ZONE != 0 {
                    above = above.max(n - g + PLANT_HEIGHT);
                    (g + 1, g)
                } else {
                    (i32::MAX, i32::MIN)
                });
            }
            if let Some(from) = run {
                claim.insert_run(z, from, hi[0]);
            }
        }
        for range in &mut ranges {
            range.1 = range.1.saturating_add(above);
        }
        self.plan.clear_columns(lo, width, ranges);
        self.plan.claim(claim);
    }
}
