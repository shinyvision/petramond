//! Where surface biomes go in climate space. The table lives in `assets/climate_table.json`, and
//! packs can layer on top of it. Its axes are temperature, humidity, continentality, erosion and
//! variance.
//!
//! It's just an ordered list of `(rectangle, biome)` rows in [`BiomeClimateIndex`]. Lowest fitness
//! distance wins and ties go to the earlier row, so the first row containing a point gets it. On
//! disk the rows are written compactly:
//!
//! ```json
//! "bands": {"temperature": {"frozen": [-1.0, -0.3], ...}, ...},
//! "groups": [
//!   {"at": {"variance": "v6"}, "rows": [
//!     {"temperature": "full", "humidity": "full", "continentality": "coast",
//!      "erosion": "e0..e1", "biome": "petramond:river"}]},
//!   {"grid": {"temperature": ["frozen", "cold"], "humidity": ["dry", "wet"]},
//!    "at": {"variance": "v6"},
//!    "rows": [{"continentality": "mid_inland..far_inland", "erosion": "e0",
//!              "biomes": [["petramond:snowy_plains", null], "petramond:plains"]}]}
//! ]
//! ```
//!
//! An axis is either a band name (`"coast"`) or a span of two (`"coast..far_inland"` runs from the
//! low edge of the first to the high edge of the second). Plain groups name one biome per row. Grid
//! groups expand each row over their temperature × humidity cells (temperature-major, then
//! humidity, then row order) and look up each cell's biome in `biomes`; `null` means no row. `at`
//! fixes axes for the whole group, and every row has to end up stating all five axes exactly once.
//! You can add an `offset`, which costs `offset²` fitness.
//!
//! Bands merge by name across layers, so if a pack retunes `frozen`, every row naming it moves.
//! Later layers' rows go first, which is how a pack gets its own biome into a niche ahead of the
//! base table. With `"replace": true` the earlier layers' rows are dropped instead.
//!
//! The temperature axis must have a `frozen` band, since its upper edge is also the sea-ice line
//! (see `density::surface`).

use std::collections::BTreeMap;

use petramond_world::biome::Biome;
use serde::Deserialize;

use super::Fingerprint;
use crate::biome::climate::{AxisRange, BiomeClimateIndex, ClimateRect};

const AXES: [&str; 5] = [
    "temperature",
    "humidity",
    "continentality",
    "erosion",
    "variance",
];
const TEMPERATURE: usize = 0;
const HUMIDITY: usize = 1;

const FROZEN_BAND: &str = "frozen";

pub struct ClimateTable {
    pub index: BiomeClimateIndex,
    pub frozen_temperature_max: f32,
    pub fingerprint: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayer {
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    bands: BTreeMap<String, BTreeMap<String, [f32; 2]>>,
    #[serde(default)]
    groups: Vec<RawGroup>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGroup {
    #[serde(default)]
    grid: Option<RawGrid>,
    #[serde(default)]
    at: RawAxes,
    rows: Vec<RawRow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGrid {
    temperature: Vec<String>,
    humidity: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAxes {
    temperature: Option<String>,
    humidity: Option<String>,
    continentality: Option<String>,
    erosion: Option<String>,
    variance: Option<String>,
}

impl RawAxes {
    fn by_axis(&self) -> [Option<&str>; 5] {
        [
            self.temperature.as_deref(),
            self.humidity.as_deref(),
            self.continentality.as_deref(),
            self.erosion.as_deref(),
            self.variance.as_deref(),
        ]
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRow {
    temperature: Option<String>,
    humidity: Option<String>,
    continentality: Option<String>,
    erosion: Option<String>,
    variance: Option<String>,
    biome: Option<String>,
    biomes: Option<RawCells>,
    #[serde(default)]
    offset: f32,
}

impl RawRow {
    fn by_axis(&self) -> [Option<&str>; 5] {
        [
            self.temperature.as_deref(),
            self.humidity.as_deref(),
            self.continentality.as_deref(),
            self.erosion.as_deref(),
            self.variance.as_deref(),
        ]
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawCells {
    Uniform(String),
    PerTemperature(Vec<RawCellRow>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawCellRow {
    Uniform(String),
    PerHumidity(Vec<Option<String>>),
}

pub(crate) struct ResolvedTable {
    pub rows: Vec<(ClimateRect, Biome)>,
    pub frozen_temperature_max: f32,
}

struct Bands([BTreeMap<String, AxisRange>; 5]);

impl Bands {
    fn merge(&mut self, raw: BTreeMap<String, BTreeMap<String, [f32; 2]>>) -> Result<(), String> {
        for (axis, bands) in raw {
            let slot = AXES
                .iter()
                .position(|a| *a == axis)
                .ok_or_else(|| format!("bands: unknown climate axis '{axis}'"))?;
            for (name, [min, max]) in bands {
                if name.is_empty() || name.contains("..") {
                    return Err(format!("bands: {axis}: invalid band name '{name}'"));
                }
                if !min.is_finite() || !max.is_finite() || min > max {
                    return Err(format!(
                        "bands: {axis} '{name}' needs finite bounds with min <= max"
                    ));
                }
                self.0[slot].insert(name, AxisRange::new(min, max));
            }
        }
        Ok(())
    }

    fn band(&self, axis: usize, name: &str) -> Result<AxisRange, String> {
        self.0[axis]
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown {} band '{name}'", AXES[axis]))
    }

    fn resolve(&self, axis: usize, spec: &str) -> Result<AxisRange, String> {
        match spec.split_once("..") {
            Some((lo, hi)) => {
                let (lo, hi) = (self.band(axis, lo)?, self.band(axis, hi)?);
                if lo.min > hi.max {
                    return Err(format!("{} span '{spec}' runs backwards", AXES[axis]));
                }
                Ok(AxisRange::new(lo.min, hi.max))
            }
            None => self.band(axis, spec),
        }
    }
}

fn biome_named(key: &str) -> Result<Biome, String> {
    Some(key)
        .filter(|key| petramond_world::registry::is_namespaced(key))
        .and_then(Biome::from_name)
        .ok_or_else(|| format!("unknown biome '{key}'"))
}

fn row_axes<'a>(
    grid_cell: Option<(&'a str, &'a str)>,
    at: [Option<&'a str>; 5],
    row: [Option<&'a str>; 5],
) -> Result<[&'a str; 5], String> {
    let mut specs = [""; 5];
    for (axis, spec) in specs.iter_mut().enumerate() {
        let from_grid = grid_cell.and_then(|(t, h)| match axis {
            TEMPERATURE => Some(t),
            HUMIDITY => Some(h),
            _ => None,
        });
        let stated: Vec<&str> = [from_grid, at[axis], row[axis]]
            .into_iter()
            .flatten()
            .collect();
        *spec = match stated.as_slice() {
            [one] => *one,
            [] => return Err(format!("a row leaves the {} axis unset", AXES[axis])),
            _ => return Err(format!("a row sets the {} axis more than once", AXES[axis])),
        };
    }
    Ok(specs)
}

fn resolve_rect(bands: &Bands, specs: [&str; 5], offset: f32) -> Result<ClimateRect, String> {
    if !offset.is_finite() {
        return Err("a row offset must be finite".into());
    }
    let mut ranges = [AxisRange::new(0.0, 0.0); 5];
    for (axis, range) in ranges.iter_mut().enumerate() {
        *range = bands.resolve(axis, specs[axis])?;
    }
    let [t, h, c, e, v] = ranges;
    Ok(ClimateRect::surface(t, h, c, e, v).with_offset(offset))
}

fn cell(cells: &RawCells, i: usize, j: usize) -> Option<&str> {
    match cells {
        RawCells::Uniform(key) => Some(key.as_str()),
        RawCells::PerTemperature(rows) => match &rows[i] {
            RawCellRow::Uniform(key) => Some(key.as_str()),
            RawCellRow::PerHumidity(keys) => keys[j].as_deref(),
        },
    }
}

fn check_cell_shape(cells: &RawCells, temps: usize, hums: usize) -> Result<(), String> {
    let RawCells::PerTemperature(rows) = cells else {
        return Ok(());
    };
    if rows.len() != temps {
        return Err(format!(
            "biomes: {} temperature rows for a {temps}-temperature grid",
            rows.len()
        ));
    }
    for row in rows {
        if let RawCellRow::PerHumidity(keys) = row {
            if keys.len() != hums {
                return Err(format!(
                    "biomes: {} humidity cells for a {hums}-humidity grid",
                    keys.len()
                ));
            }
        }
    }
    Ok(())
}

fn resolve_group(bands: &Bands, group: &RawGroup) -> Result<Vec<(ClimateRect, Biome)>, String> {
    let at = group.at.by_axis();
    let mut out = Vec::new();
    let Some(grid) = &group.grid else {
        for row in &group.rows {
            if row.biomes.is_some() {
                return Err("a plain group row names `biome`, not a `biomes` grid".into());
            }
            let key = row
                .biome
                .as_deref()
                .ok_or("a plain group row needs a `biome`")?;
            let rect = resolve_rect(bands, row_axes(None, at, row.by_axis())?, row.offset)?;
            out.push((rect, biome_named(key)?));
        }
        return Ok(out);
    };
    let (temps, hums) = (&grid.temperature, &grid.humidity);
    if temps.is_empty() || hums.is_empty() {
        return Err("a grid needs at least one temperature and one humidity band".into());
    }
    for row in &group.rows {
        if row.biome.is_some() {
            return Err("a grid group row names `biomes`, not one `biome`".into());
        }
        check_cell_shape(
            row.biomes
                .as_ref()
                .ok_or("a grid group row needs `biomes`")?,
            temps.len(),
            hums.len(),
        )?;
    }
    for (i, t) in temps.iter().enumerate() {
        for (j, h) in hums.iter().enumerate() {
            for row in &group.rows {
                let cells = row.biomes.as_ref().expect("checked above");
                let Some(key) = cell(cells, i, j) else {
                    continue;
                };
                let specs = row_axes(Some((t.as_str(), h.as_str())), at, row.by_axis())?;
                out.push((resolve_rect(bands, specs, row.offset)?, biome_named(key)?));
            }
        }
    }
    Ok(out)
}

pub(crate) fn parse_layers(texts: &[&str]) -> Result<ResolvedTable, String> {
    let layers = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            serde_json::from_str::<RawLayer>(text).map_err(|e| format!("layer #{i}: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut bands = Bands(Default::default());
    let mut groups_by_layer: Vec<Vec<RawGroup>> = Vec::new();
    for (i, layer) in layers.into_iter().enumerate() {
        bands
            .merge(layer.bands)
            .map_err(|e| format!("layer #{i}: {e}"))?;
        if layer.replace {
            groups_by_layer.clear();
        }
        groups_by_layer.push(layer.groups);
    }
    let mut rows = Vec::new();
    for groups in groups_by_layer.iter().rev() {
        for (g, group) in groups.iter().enumerate() {
            rows.extend(resolve_group(&bands, group).map_err(|e| format!("group #{g}: {e}"))?);
        }
    }
    if rows.is_empty() {
        return Err("the table has no rows: every climate needs a biome".into());
    }
    let frozen_temperature_max = bands
        .band(TEMPERATURE, FROZEN_BAND)
        .map_err(|e| format!("{e}: its upper edge is the sea-ice line"))?
        .max;
    Ok(ResolvedTable {
        rows,
        frozen_temperature_max,
    })
}

impl ResolvedTable {
    fn fingerprint(&self) -> u64 {
        let mut hash = Fingerprint::new();
        hash.eat(&self.frozen_temperature_max.to_le_bytes());
        for (rect, biome) in &self.rows {
            for range in rect.axis_ranges() {
                hash.eat(&range.min.to_le_bytes());
                hash.eat(&range.max.to_le_bytes());
            }
            hash.eat(&rect.offset().to_le_bytes());
            hash.eat(biome.key().as_bytes());
        }
        hash.finish()
    }

    fn build(self) -> ClimateTable {
        ClimateTable {
            fingerprint: self.fingerprint(),
            index: BiomeClimateIndex::from_rects(&self.rows),
            frozen_temperature_max: self.frozen_temperature_max,
        }
    }
}

pub(crate) static TABLE: petramond_world::content::Slot<ClimateTable> =
    petramond_world::content::Slot::new(
        "climate_table.json",
        &[petramond_world::content::stage::BIOMES],
        load_table,
    );

fn load_table(reg: &petramond_world::content::ContentRegistry) -> Result<ClimateTable, String> {
    petramond_world::registry::read_catalog(
        reg.packs(),
        "climate_table.json",
        "climate table",
        parse_layers,
    )
    .map(|table| table.build())
}

pub fn table() -> &'static ClimateTable {
    TABLE.current()
}

#[cfg(test)]
mod tests;
