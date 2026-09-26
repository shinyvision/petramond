//! Underground vein table — a layered catalog (`assets/ores.json`): the ore
//! veins and dirt / gravel / tuff / marble blobs the underground scatter pass
//! places (see [`crate::feature::scatter`]).
//!
//! ```json
//! {"ore": "petramond:diamond_ore", "block": "petramond:diamond_ore",
//!  "salt": 10551318, "count": 7, "shape": {"grid3": {"max_ore": 9}},
//!  "y": [-64, 16], "depth_ramp": 1.0, "hosts": ["petramond:stone"]}
//! ```
//!
//! A row places up to `count` veins of `block` per chunk column, their
//! origins uniform over the world-Y band `y`, each overwriting only cells
//! that currently hold one of its `hosts` (default: stone). `shape` is a
//! `blob` of `~size` cells or a `grid3` single-layer 3×3 patch of
//! `1..=max_ore` cells. `depth_ramp` accepts each rolled vein with a chance
//! of `depth_ramp · t²`, `t` rising from 0 at the band top to 1 at its floor.
//! Vein positions derive from the row's `salt`, never its index.
//!
//! Engine rows own the low ids in the frozen placement order below; a pack
//! OVERRIDES an engine row to retune it or ADDS a vein under its own
//! namespaced key, placed after the engine rows in load order — so pack ores
//! only claim host cells the engine veins left, and never move them.

use std::sync::LazyLock;

use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use serde::Deserialize;

/// Engine vein names in frozen placement order. Marble is last so it only
/// claims stone every ore left: the ore counts are tuned and must not move
/// because a stone flavour was added.
const ENGINE_ORE_NAMES: &[&str] = &[
    "petramond:dirt",
    "petramond:gravel",
    "petramond:tuff",
    "petramond:coal_ore",
    "petramond:copper_ore",
    "petramond:iron_ore",
    "petramond:gold_ore",
    "petramond:diamond_ore",
    "petramond:marble",
];

/// Widest reach a vein may have from its rolled origin: the scatter pass
/// regenerates only the 3×3 column neighbourhood around a section, so a
/// vein must stay inside its origin column's neighbours to be seamless.
const MAX_VEIN_REACH: i32 = 16;

/// How a vein materialises its cells around the rolled origin.
#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum VeinShape {
    /// Roughly-spherical blob of `~size` cells with per-vein radius jitter.
    Blob { size: i32 },
    /// One horizontal 3×3 layer centred on the origin holding exactly
    /// `1..=max_ore` ore cells (uniformly chosen among the 9 slots).
    Grid3 { max_ore: i32 },
}

impl VeinShape {
    /// Conservative `(horizontal, vertical)` reach of one vein from its rolled
    /// origin, in cells: every write lands within this Chebyshev box. Blob radius
    /// is `base_r × (0.85 + 0.4·f)` with `f < 1`, so `ceil(base_r × 1.25)` bounds
    /// `ceil(r)` (f32 multiply is monotone; an exact-integer bound still holds
    /// because `r` is strictly below it). Grid3 writes one 3×3 layer.
    pub fn reach(self) -> (i32, i32) {
        match self {
            VeinShape::Blob { size } => {
                let r = (blob_base_radius(size) * 1.25).ceil() as i32;
                (r, r)
            }
            VeinShape::Grid3 { .. } => (1, 0),
        }
    }
}

/// Radius for a blob of `size` cells: `r = cbrt(3·size / 4π)` — shared by the
/// materialiser and the reach bound so they can never drift apart.
#[inline]
pub fn blob_base_radius(size: i32) -> f32 {
    petramond_math::detmath::cbrtf((size as f32) * 3.0 / (4.0 * std::f32::consts::PI))
}

/// One loaded vein row.
#[derive(Clone, Debug, PartialEq)]
pub struct OreVein {
    pub block: Block,
    pub salt: u64,
    pub count: i32,
    pub shape: VeinShape,
    pub y_min: i32,
    pub y_max: i32,
    /// Peak acceptance chance of the depth ramp, `None` = every vein placed.
    pub depth_ramp: Option<f32>,
    /// The blocks a vein may overwrite.
    pub hosts: &'static [Block],
}

/// The loaded vein table plus the bounds the scatter pass culls with.
pub struct OreTable {
    /// Every vein row, in placement order.
    pub veins: &'static [OreVein],
    /// The widest horizontal reach over every row.
    pub max_reach: i32,
    /// World-Y span any vein can write: the union of every row's band
    /// widened by its vertical reach, clamped to the world.
    pub y_span: (i32, i32),
}

/// One vein row as written in `ores.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOre {
    ore: String,
    block: Block,
    salt: u64,
    count: i32,
    shape: VeinShape,
    y: (i32, i32),
    #[serde(default)]
    depth_ramp: Option<f32>,
    #[serde(default = "default_hosts")]
    hosts: Vec<Block>,
}

fn default_hosts() -> Vec<Block> {
    vec![Block::Stone]
}

#[derive(Deserialize)]
struct RawFile {
    ores: Vec<RawOre>,
}

impl RawOre {
    fn resolve(self) -> Result<OreVein, String> {
        let (y_min, y_max) = self.y;
        if !(0..=256).contains(&self.count) {
            return Err(format!("count {} outside 0..=256", self.count));
        }
        if y_min > y_max || y_min < WORLD_MIN_Y || y_max > WORLD_MAX_Y {
            return Err(format!(
                "y band [{y_min}, {y_max}] must ascend inside [{WORLD_MIN_Y}, {WORLD_MAX_Y}]"
            ));
        }
        match self.shape {
            VeinShape::Blob { size } if size < 1 => {
                return Err(format!("blob size {size} must be at least 1"));
            }
            VeinShape::Grid3 { max_ore } if !(1..=9).contains(&max_ore) => {
                return Err(format!("grid3 max_ore {max_ore} outside 1..=9"));
            }
            _ => {}
        }
        let (reach, _) = self.shape.reach();
        if reach > MAX_VEIN_REACH {
            return Err(format!(
                "a vein reaching {reach} blocks exceeds the {MAX_VEIN_REACH}-block \
                 column neighbourhood the scatter pass regenerates"
            ));
        }
        if let Some(chance) = self.depth_ramp {
            if !chance.is_finite() || !(0.0..=1.0).contains(&chance) || y_min == y_max {
                return Err("depth_ramp needs a chance in 0..=1 over a band of height".into());
            }
        }
        if self.hosts.is_empty() {
            return Err("hosts: a vein needs at least one block to overwrite".into());
        }
        Ok(OreVein {
            block: self.block,
            salt: self.salt,
            count: self.count,
            shape: self.shape,
            y_min,
            y_max,
            depth_ramp: self.depth_ramp,
            hosts: self.hosts.leak(),
        })
    }
}

impl OreTable {
    pub(crate) fn new(veins: &'static [OreVein]) -> Self {
        let max_reach = veins.iter().map(|v| v.shape.reach().0).max().unwrap_or(0);
        let low = veins.iter().map(|v| v.y_min - v.shape.reach().1).min();
        let high = veins.iter().map(|v| v.y_max + v.shape.reach().1).max();
        let y_span = match (low, high) {
            (Some(low), Some(high)) => (low.max(WORLD_MIN_Y), high.min(WORLD_MAX_Y)),
            // No veins: an empty span no section overlaps.
            _ => (WORLD_MAX_Y, WORLD_MIN_Y),
        };
        Self {
            veins,
            max_reach,
            y_span,
        }
    }
}

fn parse_layers(texts: &[&str]) -> Result<OreTable, String> {
    let catalog = petramond_world::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.ores),
        |r| &r.ore,
        ENGINE_ORE_NAMES,
        "ore vein",
        |r, _, _| {
            let name = r.ore.clone();
            r.resolve().map_err(|e| format!("ore vein '{name}': {e}"))
        },
    )?;
    Ok(OreTable::new(catalog.rows()))
}

/// The loaded vein table. Loads once; a missing or malformed layer fails
/// loudly at first use.
pub fn table() -> &'static OreTable {
    static TABLE: LazyLock<OreTable> = LazyLock::new(|| {
        petramond_world::registry::read_catalog("ores.json", "ore vein", parse_layers)
    });
    &TABLE
}

#[cfg(test)]
pub(crate) mod tests;
