//! Bounded excavation requests, independent of underground habitat assignment.

use super::underground::UndergroundBiomes;
use crate::noise::settings::{CAVE_LATTICE_STEP, CAVE_MIN_Y};
use serde::Deserialize;
use std::sync::LazyLock;

pub mod effects;
mod field;
mod load;
pub use field::{Bounds, ExtentProbe, FieldShape, RawBounds, RawFieldShape};
#[cfg(test)]
mod tests;

/// Geometry shared by every placement of a chamber.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Chamber {
    pub r_min: i32,
    pub r_max: i32,
    pub flatten: f64,
    pub stretch: (f64, f64),
    pub sill: f64,
    pub feather: f64,
    pub strength: f64,
    pub lobes: i32,
    pub lobe_spread: f64,
    pub lobe_scale: (f64, f64),
    pub tunnel: f64,
    pub rim_noise: f64,
}

/// Candidate distribution and optional admission condition, not a voxel mask.
pub struct Placement {
    pub spacing: i32,
    pub one_in: i32,
    pub y: (i32, i32),
    pub underground_biome: Option<u8>,
    pub contact: Option<Contact>,
}

/// A required intersection with an independent geometry source.
#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Contact {
    NaturalCave,
}

pub struct Excavation {
    pub name: &'static str,
    pub salt: u64,
    pub placement: Placement,
    pub shape: ExcavationShape,
    pub connections: Option<Connections>,
}

pub enum ExcavationShape {
    Chamber(Chamber),
    Field(Box<FieldShape>),
}

impl Excavation {
    pub fn chamber(&self) -> Option<&Chamber> {
        match &self.shape {
            ExcavationShape::Chamber(chamber) => Some(chamber),
            ExcavationShape::Field(_) => None,
        }
    }

    pub fn field(&self) -> Option<&FieldShape> {
        match &self.shape {
            ExcavationShape::Field(field) => Some(field),
            ExcavationShape::Chamber(_) => None,
        }
    }

    pub fn y_span(&self) -> (i32, i32) {
        self.field().map_or(self.placement.y, |f| (f.y[0], f.y[1]))
    }
}

/// Curved passages between admitted rooms in adjacent placement cells.
#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connections {
    pub radius: [f64; 2],
    pub flatten: f64,
    pub feather: f64,
    /// Lateral control-point offset as a fraction of the endpoint distance.
    pub bend: f64,
}

impl Connections {
    pub fn reach_y(&self) -> i32 {
        (self.radius[1] * self.flatten + self.feather).ceil() as i32
    }

    pub fn gather_pad(&self, spacing: i32) -> i32 {
        // Adjacent jittered cells have endpoints at most sqrt(5) * spacing apart.
        spacing + (3.0 * spacing as f64 * self.bend + self.radius[1] + self.feather).ceil() as i32
    }
}

pub struct Excavations {
    pub field_y_span: Option<(i32, i32)>,
    pub surface_offset: i32,
    pub rows: Vec<&'static Excavation>,
    pub y_span: Option<(i32, i32)>,
    pub fingerprint: u64,
}

pub fn table() -> &'static Excavations {
    static TABLE: LazyLock<Excavations> = LazyLock::new(|| {
        petramond_world::registry::read_catalog("excavations.json", "excavation", |texts| {
            load::parse_layers(texts, super::underground::table())
        })
    });
    &TABLE
}

#[cfg(test)]
pub fn test_table(layers: &[&str], biomes: &UndergroundBiomes) -> &'static Excavations {
    Box::leak(Box::new(
        load::parse_layers(layers, biomes).expect("synthetic excavations"),
    ))
}

impl Chamber {
    /// Vertical radius for a rolled horizontal radius. Floored at one cell so a
    /// very flat row still has a room rather than a plane.
    #[inline]
    pub fn ry(&self, rx: i32) -> f64 {
        (rx as f64 * self.flatten).max(1.0)
    }

    /// How far a satellite lobe can push a room's surface past the primary's,
    /// as a multiple of the primary's radii. Never below 1: the primary lobe
    /// is always there.
    #[inline]
    pub fn spread_factor(&self) -> f64 {
        if self.lobes > 1 {
            (self.lobe_spread + self.lobe_scale.1).max(1.0)
        } else {
            1.0
        }
    }

    /// How far below its centre a room reaches (the sill cut) and how far
    /// above (the lobes plus their rim). Every depth bound rests on these, so
    /// a profile that is not exactly zero past them is a correctness bug.
    #[inline]
    pub fn drop(&self, rx: i32) -> i32 {
        (self.ry(rx) * self.sill).round() as i32
    }

    #[inline]
    pub fn rise(&self, rx: i32) -> i32 {
        (self.ry(rx) * self.spread_factor() + self.feather).ceil() as i32
    }

    /// Furthest from its centre a rolled room's non-zero influence can reach
    /// horizontally — the candidate window's only pad.
    #[inline]
    pub fn reach_xz(&self) -> i32 {
        let extent = if self.lobes > 1 {
            (self.lobe_spread + self.lobe_scale.1 * self.stretch.1).max(self.stretch.1)
        } else {
            self.stretch.1
        };
        (self.r_max as f64 * extent + self.feather).ceil() as i32
    }

    /// Worst-case vertical extent over every radius the row can roll. Not
    /// monotone in `rx` once [`Self::ry`]'s floor bites, so it is taken by
    /// scanning rather than evaluated at `r_max`.
    pub fn extent_y(&self) -> (i32, i32) {
        (self.r_min..=self.r_max).fold((0, 0), |(d, r), rx| {
            (d.max(self.drop(rx)), r.max(self.rise(rx)))
        })
    }

    /// The depth band a room's CENTRE may be rolled in: the row's own band,
    /// clipped to the range the carvers actually cut. Nothing is carved below
    /// `CAVE_MIN_Y`, so a room rolled under it is not tapered by its own sill
    /// but sliced by the world floor — a dead-flat plane of bare rock, since
    /// the lining shell needs the same `interior` gate the carve does.
    #[inline]
    pub fn placement_band(band: (i32, i32)) -> (i32, i32) {
        (band.0.max(CAVE_MIN_Y), band.1)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChamber {
    radius: [i32; 2],
    #[serde(default = "default_flatten")]
    flatten: f64,
    #[serde(default = "default_lobe_scale")]
    stretch: [f64; 2],
    #[serde(default = "default_sill")]
    sill: f64,
    feather: f64,
    #[serde(default = "one")]
    strength: f64,
    #[serde(default = "one_i32")]
    lobes: i32,
    #[serde(default)]
    lobe_spread: f64,
    #[serde(default = "default_lobe_scale")]
    lobe_scale: [f64; 2],
    #[serde(default)]
    tunnel: f64,
    #[serde(default)]
    rim_noise: f64,
}

fn one() -> f64 {
    1.0
}

fn one_i32() -> i32 {
    1
}

fn default_lobe_scale() -> [f64; 2] {
    [1.0, 1.0]
}

fn default_flatten() -> f64 {
    0.6
}

fn default_sill() -> f64 {
    0.7
}
