//! The surface biome placement table — a layered catalog
//! (`assets/climate_table.json`): which biome the classifier picks for a
//! point in the five-axis climate space (temperature, humidity,
//! continentality, erosion, variance).
//!
//! The table is a flat, ORDERED list of `(rectangle, biome)` rows served by
//! [`BiomeClimateIndex`]: the lowest fitness distance wins and row order
//! breaks ties, so the first row containing a point claims it. The file
//! writes those rows compactly:
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
//! An axis names a BAND of that axis (`"coast"`) or a span of two
//! (`"coast..far_inland"`, the low edge of the first to the high edge of the
//! second). A plain group's rows each name one biome; a grid group expands
//! every row across its temperature × humidity cells — temperature-major,
//! then humidity, then the group's rows in order — taking each cell's biome
//! from `biomes` (one key for every cell, or one entry per temperature that
//! is a key for the whole row or a per-humidity list; `null` emits no row).
//! `at` fixes axes for every row of the group, and every row states each of
//! the five axes exactly once between the grid, `at` and itself. A row may
//! carry an `offset` fitness penalty (added as `offset²`).
//!
//! Layering: bands merge by name across layers (a pack retuning `frozen`
//! moves every row that names it), then rows resolve against the merged
//! bands. A later layer's rows go FIRST, so a pack row claims the climate it
//! contains ahead of the base table — the way a pack places its own biome in
//! a niche; `"replace": true` drops every earlier layer's rows instead.
//!
//! The temperature axis must define a `frozen` band: its upper edge is also
//! the sea-ice line (see `density::surface`).

use std::collections::BTreeMap;

use petramond_world::biome::Biome;
use serde::Deserialize;

use super::Fingerprint;
use crate::biome::climate::{AxisRange, BiomeClimateIndex, ClimateRect};

/// The climate axes, in [`ClimateRect`] order.
const AXES: [&str; 5] = [
    "temperature",
    "humidity",
    "continentality",
    "erosion",
    "variance",
];
const TEMPERATURE: usize = 0;
const HUMIDITY: usize = 1;

/// The temperature band whose upper edge is the sea-ice line.
const FROZEN_BAND: &str = "frozen";

/// The loaded placement table.
pub struct ClimateTable {
    pub index: BiomeClimateIndex,
    /// Upper edge of the `frozen` temperature band: colder shallow water
    /// freezes over.
    pub frozen_temperature_max: f32,
    /// Hash of the resolved rows, stamped into the column-gen cache: a pack
    /// that moves a biome must not be served stale cached columns.
    pub fingerprint: u64,
}

/// One layer of `climate_table.json`.
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
    /// A plain group row's biome.
    biome: Option<String>,
    /// A grid group row's per-cell biomes.
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

/// A grid row's biomes: one key for every cell, or one entry per
/// temperature.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawCells {
    Uniform(String),
    PerTemperature(Vec<RawCellRow>),
}

/// One temperature's cells: one key for the whole row, or one per humidity
/// (`null` = no row for that cell).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawCellRow {
    Uniform(String),
    PerHumidity(Vec<Option<String>>),
}

/// The layers merged and resolved: the ordered rows the index is built from.
pub(crate) struct ResolvedTable {
    pub rows: Vec<(ClimateRect, Biome)>,
    pub frozen_temperature_max: f32,
}

/// Every axis's bands, merged across layers.
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

    /// A band (`"coast"`) or a span of two (`"coast..far_inland"`).
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

/// The five axis specs of one row: each stated exactly once between the
/// grid (temperature and humidity of a grid group), the group's `at` and
/// the row itself.
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

/// The biome of grid cell `(i, j)`, `None` for a `null` cell.
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

/// Merge and resolve the layers (base first, packs after).
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

/// The placement table stage (see [`super::content_stages`]); a missing or
/// malformed layer fails the registry build.
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

/// The current registry's placement table.
pub fn table() -> &'static ClimateTable {
    TABLE.current()
}

#[cfg(test)]
mod tests;
