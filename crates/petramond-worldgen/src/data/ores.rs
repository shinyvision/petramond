//! Underground vein table (`assets/ores.json`): ore veins and dirt/gravel/tuff/marble blobs placed
//! by the underground scatter pass (see [`crate::feature::scatter`]).
//!
//! ```json
//! {"ore": "petramond:diamond_ore", "block": "petramond:diamond_ore",
//!  "salt": 10551318, "count": 7, "shape": {"grid3": {"max_ore": 9}},
//!  "y": [-64, 16], "depth_ramp": 1.0, "hosts": ["petramond:stone"]}
//! ```
//!
//! Row places up to `count` veins of `block` per column, origins uniform over Y band `y`, only
//! overwriting cells in `hosts` (default stone). `shape` is a `blob` of `~size` cells or a
//! `grid3` 3x3 patch of `1..=max_ore` cells. Each vein is kept with chance `depth_ramp * t^2`,
//! t from 0 at the top of the band to 1 at the bottom. Positions come from `salt`, not row index.
//!
//! Engine rows below keep fixed low ids. Packs can override an engine row to retune it, or add
//! their own under a namespaced key, after the engine rows in load order. Pack ores only get
//! leftover host cells and never displace engine veins.

use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use serde::Deserialize;

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

const MAX_VEIN_REACH: i32 = 16;

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum VeinShape {
    Blob { size: i32 },
    Grid3 { max_ore: i32 },
}

impl VeinShape {
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

#[inline]
pub fn blob_base_radius(size: i32) -> f32 {
    petramond_math::detmath::cbrtf((size as f32) * 3.0 / (4.0 * std::f32::consts::PI))
}

#[derive(Clone, Debug, PartialEq)]
pub struct OreVein {
    pub block: Block,
    pub salt: u64,
    pub count: i32,
    pub shape: VeinShape,
    pub y_min: i32,
    pub y_max: i32,
    pub depth_ramp: Option<f32>,
    pub hosts: &'static [Block],
}

pub struct OreTable {
    pub veins: &'static [OreVein],
    pub max_reach: i32,
    pub y_span: (i32, i32),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOre {
    ore: String,
    block: Block,
    #[serde(default)]
    salt: Option<u64>,
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
            salt: self
                .salt
                .unwrap_or_else(|| crate::salts::named("ore", &self.ore)),
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
    let mut salt_owners = std::collections::HashMap::new();
    let catalog = petramond_world::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.ores),
        |r| &r.ore,
        ENGINE_ORE_NAMES,
        "ore vein",
        |r, _, _| {
            let name = r.ore.clone();
            let vein = r.resolve().map_err(|e| format!("ore vein '{name}': {e}"))?;
            if let Some(previous) = salt_owners.insert(vein.salt, name.clone()) {
                return Err(format!(
                    "ore veins '{previous}' and '{name}' share salt {}",
                    vein.salt
                ));
            }
            Ok(vein)
        },
    )?;
    Ok(OreTable::new(catalog.rows()))
}

pub(crate) static TABLE: petramond_world::content::Slot<OreTable> =
    petramond_world::content::Slot::new(
        "ores.json",
        &[petramond_world::content::stage::BLOCKS],
        load_table,
    );

fn load_table(reg: &petramond_world::content::ContentRegistry) -> Result<OreTable, String> {
    petramond_world::registry::read_catalog(reg.packs(), "ores.json", "ore vein", parse_layers)
}

pub fn table() -> &'static OreTable {
    TABLE.current()
}

#[cfg(test)]
mod tests;
