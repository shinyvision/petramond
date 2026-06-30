//! Parameter-space surface biome table.
//!
//! Surface biomes are assigned by nearest climate rectangle (see
//! [`super::climate::BiomeClimateIndex`]). Rather than hand-tuning one rectangle
//! per biome, this module generates a dense table of `(rectangle, biome)` rows
//! that tile the five climate axes (temperature, humidity, continentality,
//! erosion, variance) the way a well-studied reference generator does: a grid of
//! base biomes selected by temperature/humidity, then sliced by erosion bands and
//! mirrored across the variance ("low"/"high") fold, with coast/ocean/peak
//! special cases layered on top.
//!
//! The grids below already hold *our* palette — the reference biome set is folded
//! into our biomes at authoring time, so the table is the single editable knob for
//! how climate maps to terrain cover.

use crate::biome::Biome;
use crate::biome::Biome::*;

use super::climate::{AxisRange, ClimateRect};

type Row = (ClimateRect, Biome);

// --- Axis bands -----------------------------------------------------------

const FULL: AxisRange = AxisRange::new(-1.0, 1.0);

/// Temperature bands, cold (index 0) to hot (index 4).
const T: [AxisRange; 5] = [
    AxisRange::new(-1.0, -0.45),
    AxisRange::new(-0.45, -0.15),
    AxisRange::new(-0.15, 0.2),
    AxisRange::new(0.2, 0.55),
    AxisRange::new(0.55, 1.0),
];

/// Humidity bands, dry (index 0) to wet (index 4).
const H: [AxisRange; 5] = [
    AxisRange::new(-1.0, -0.35),
    AxisRange::new(-0.35, -0.1),
    AxisRange::new(-0.1, 0.1),
    AxisRange::new(0.1, 0.3),
    AxisRange::new(0.3, 1.0),
];

/// Erosion bands, least eroded (index 0) to most eroded (index 6).
const E: [AxisRange; 7] = [
    AxisRange::new(-1.0, -0.78),
    AxisRange::new(-0.78, -0.375),
    AxisRange::new(-0.375, -0.2225),
    AxisRange::new(-0.2225, 0.05),
    AxisRange::new(0.05, 0.45),
    AxisRange::new(0.45, 0.55),
    AxisRange::new(0.55, 1.0),
];

// Continentality bands, ocean-ward to deep-inland.
const MUSHROOM: AxisRange = AxisRange::new(-1.2, -1.05);
const DEEP_OCEAN_C: AxisRange = AxisRange::new(-1.05, -0.455);
const OCEAN_C: AxisRange = AxisRange::new(-0.455, -0.19);
const COAST: AxisRange = AxisRange::new(-0.19, -0.11);
const NEAR_INLAND: AxisRange = AxisRange::new(-0.11, 0.03);
const MID_INLAND: AxisRange = AxisRange::new(0.03, 0.3);
const FAR_INLAND: AxisRange = AxisRange::new(0.3, 1.0);

const UNFROZEN: AxisRange = span(T[1], T[4]);

/// Variance slice edges; the inland builder walks adjacent pairs to form 13
/// variance ranges that mirror around zero (the "low"/"high" fold).
const VARIANCE_EDGES: [f32; 14] = [
    -1.0,
    -0.93333334,
    -0.7666667,
    -0.56666666,
    -0.4,
    -0.26666668,
    -0.05,
    0.05,
    0.26666668,
    0.4,
    0.56666666,
    0.7666667,
    0.93333334,
    1.0,
];

/// A union span from the low edge of `a` to the high edge of `b`.
const fn span(a: AxisRange, b: AxisRange) -> AxisRange {
    AxisRange::new(a.min, b.max)
}

/// The low side of the variance fold (the reference's negative-variance half).
fn is_low(variance: AxisRange) -> bool {
    variance.max < 0.0
}

// --- Palette grids (temperature row, humidity column) ---------------------

const CORE: [[Biome; 5]; 5] = [
    [SnowyTundra, SnowyTundra, SnowyTundra, SnowyTaiga, Taiga],
    [Plains, Plains, Forest, Taiga, OldGrowthTaiga],
    [Forest, Plains, Forest, Forest, Forest],
    [Savanna, Savanna, Forest, Forest, Forest],
    [Desert, Desert, Desert, Desert, Desert],
];

const CORE_HIGH: [[Option<Biome>; 5]; 5] = [
    [Some(SnowyTundra), None, Some(SnowyTaiga), None, None],
    [None, None, None, None, Some(RedwoodForest)],
    [Some(Plains), None, None, Some(Forest), None],
    [None, None, Some(Plains), Some(Forest), Some(Forest)],
    [None, None, None, None, None],
];

const PLATEAU: [[Biome; 5]; 5] = [
    [
        SnowyTundra,
        SnowyTundra,
        SnowyTundra,
        SnowyTaiga,
        SnowyTaiga,
    ],
    [Meadow, Meadow, Forest, Taiga, OldGrowthTaiga],
    [Meadow, Meadow, Meadow, Meadow, Forest],
    [Savanna, Savanna, Forest, Forest, Forest],
    [Desert, Desert, Desert, Desert, Desert],
];

const PLATEAU_HIGH: [[Option<Biome>; 5]; 5] = [
    [Some(SnowyTundra), None, None, None, None],
    [None, None, Some(Meadow), Some(Meadow), Some(RedwoodForest)],
    [None, None, Some(Forest), Some(Forest), None],
    [None, None, None, None, None],
    [Some(Desert), Some(Desert), None, None, None],
];

const HILLS: [[Option<Biome>; 5]; 5] = [
    [Some(WindsweptHills); 5],
    [Some(WindsweptHills); 5],
    [Some(WindsweptHills); 5],
    [None; 5],
    [None; 5],
];

// Ocean by temperature: deep row (no deep variant of the warmest), shallow row.
const OCEANS: [[Biome; 5]; 2] = [
    [DeepOcean, DeepOcean, DeepOcean, DeepOcean, Ocean],
    [Ocean, Ocean, Ocean, Ocean, Ocean],
];

// --- Pickers (temperature index `i`, humidity index `j`, variance slice `v`) --

fn pick_core(i: usize, j: usize, v: AxisRange) -> Biome {
    if is_low(v) {
        CORE[i][j]
    } else {
        CORE_HIGH[i][j].unwrap_or(CORE[i][j])
    }
}

fn pick_core_or_arid_if_hot(i: usize, j: usize, v: AxisRange) -> Biome {
    if i == 4 {
        Desert
    } else {
        pick_core(i, j, v)
    }
}

fn pick_core_or_arid_if_hot_or_slope_if_cold(i: usize, j: usize, v: AxisRange) -> Biome {
    if i == 0 {
        pick_slope(i, j, v)
    } else {
        pick_core_or_arid_if_hot(i, j, v)
    }
}

fn maybe_windswept_open(i: usize, j: usize, v: AxisRange, fallback: Biome) -> Biome {
    if i > 1 && j < 4 && !is_low(v) {
        Savanna
    } else {
        fallback
    }
}

fn pick_windswept_coast(i: usize, j: usize, v: AxisRange) -> Biome {
    let base = if !is_low(v) {
        pick_core(i, j, v)
    } else {
        pick_beach(i)
    };
    maybe_windswept_open(i, j, v, base)
}

fn pick_beach(i: usize) -> Biome {
    if i == 4 {
        Desert
    } else {
        Beach
    }
}

fn pick_plateau(i: usize, j: usize, v: AxisRange) -> Biome {
    if is_low(v) {
        PLATEAU[i][j]
    } else {
        PLATEAU_HIGH[i][j].unwrap_or(PLATEAU[i][j])
    }
}

fn pick_peak(i: usize, _j: usize, _v: AxisRange) -> Biome {
    if i <= 2 {
        SnowyPeaks
    } else if i == 3 {
        StonyPeaks
    } else {
        Desert
    }
}

fn pick_slope(i: usize, j: usize, v: AxisRange) -> Biome {
    if i >= 3 {
        pick_plateau(i, j, v)
    } else if j <= 1 {
        SnowySlopes
    } else {
        Grove
    }
}

fn pick_hills(i: usize, j: usize, v: AxisRange) -> Biome {
    HILLS[i][j].unwrap_or_else(|| pick_core(i, j, v))
}

// --- Table assembly -------------------------------------------------------

fn add(
    rows: &mut Vec<Row>,
    t: AxisRange,
    h: AxisRange,
    c: AxisRange,
    e: AxisRange,
    v: AxisRange,
    biome: Biome,
) {
    rows.push((ClimateRect::surface(t, h, c, e, v), biome));
}

fn add_off_coast(rows: &mut Vec<Row>) {
    add(rows, FULL, FULL, MUSHROOM, FULL, FULL, Plains);
    for i in 0..5 {
        add(rows, T[i], FULL, DEEP_OCEAN_C, FULL, FULL, OCEANS[0][i]);
        add(rows, T[i], FULL, OCEAN_C, FULL, FULL, OCEANS[1][i]);
    }
}

fn add_peaks(rows: &mut Vec<Row>, v: AxisRange) {
    for i in 0..5 {
        let t = T[i];
        for j in 0..5 {
            let h = H[j];
            let core = pick_core(i, j, v);
            let core_arid = pick_core_or_arid_if_hot(i, j, v);
            let core_arid_slope = pick_core_or_arid_if_hot_or_slope_if_cold(i, j, v);
            let plateau = pick_plateau(i, j, v);
            let hills = pick_hills(i, j, v);
            let windswept = maybe_windswept_open(i, j, v, hills);
            let peak = pick_peak(i, j, v);
            add(rows, t, h, span(COAST, FAR_INLAND), E[0], v, peak);
            add(
                rows,
                t,
                h,
                span(COAST, NEAR_INLAND),
                E[1],
                v,
                core_arid_slope,
            );
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[1], v, peak);
            add(
                rows,
                t,
                h,
                span(COAST, NEAR_INLAND),
                span(E[2], E[3]),
                v,
                core,
            );
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[2], v, plateau);
            add(rows, t, h, MID_INLAND, E[3], v, core_arid);
            add(rows, t, h, FAR_INLAND, E[3], v, plateau);
            add(rows, t, h, span(COAST, FAR_INLAND), E[4], v, core);
            add(rows, t, h, span(COAST, NEAR_INLAND), E[5], v, windswept);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[5], v, hills);
            add(rows, t, h, span(COAST, FAR_INLAND), E[6], v, core);
        }
    }
}

fn add_high_slice(rows: &mut Vec<Row>, v: AxisRange) {
    for i in 0..5 {
        let t = T[i];
        for j in 0..5 {
            let h = H[j];
            let core = pick_core(i, j, v);
            let core_arid = pick_core_or_arid_if_hot(i, j, v);
            let core_arid_slope = pick_core_or_arid_if_hot_or_slope_if_cold(i, j, v);
            let plateau = pick_plateau(i, j, v);
            let hills = pick_hills(i, j, v);
            let windswept = maybe_windswept_open(i, j, v, core);
            let slope = pick_slope(i, j, v);
            let peak = pick_peak(i, j, v);
            add(rows, t, h, COAST, span(E[0], E[1]), v, core);
            add(rows, t, h, NEAR_INLAND, E[0], v, slope);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[0], v, peak);
            add(rows, t, h, NEAR_INLAND, E[1], v, core_arid_slope);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[1], v, slope);
            add(
                rows,
                t,
                h,
                span(COAST, NEAR_INLAND),
                span(E[2], E[3]),
                v,
                core,
            );
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[2], v, plateau);
            add(rows, t, h, MID_INLAND, E[3], v, core_arid);
            add(rows, t, h, FAR_INLAND, E[3], v, plateau);
            add(rows, t, h, span(COAST, FAR_INLAND), E[4], v, core);
            add(rows, t, h, span(COAST, NEAR_INLAND), E[5], v, windswept);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[5], v, hills);
            add(rows, t, h, span(COAST, FAR_INLAND), E[6], v, core);
        }
    }
}

fn add_mid_slice(rows: &mut Vec<Row>, v: AxisRange) {
    add(rows, FULL, FULL, COAST, span(E[0], E[2]), v, Beach);
    add(
        rows,
        UNFROZEN,
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Swamp,
    );

    for i in 0..5 {
        let t = T[i];
        for j in 0..5 {
            let h = H[j];
            let core = pick_core(i, j, v);
            let core_arid = pick_core_or_arid_if_hot(i, j, v);
            let core_arid_slope = pick_core_or_arid_if_hot_or_slope_if_cold(i, j, v);
            let hills = pick_hills(i, j, v);
            let plateau = pick_plateau(i, j, v);
            let beach = pick_beach(i);
            let windswept = maybe_windswept_open(i, j, v, core);
            let windswept_coast = pick_windswept_coast(i, j, v);
            let slope = pick_slope(i, j, v);
            add(rows, t, h, span(NEAR_INLAND, FAR_INLAND), E[0], v, slope);
            add(
                rows,
                t,
                h,
                span(NEAR_INLAND, MID_INLAND),
                E[1],
                v,
                core_arid_slope,
            );
            add(
                rows,
                t,
                h,
                FAR_INLAND,
                E[1],
                v,
                if i == 0 { slope } else { plateau },
            );
            add(rows, t, h, NEAR_INLAND, E[2], v, core);
            add(rows, t, h, MID_INLAND, E[2], v, core_arid);
            add(rows, t, h, FAR_INLAND, E[2], v, plateau);
            add(rows, t, h, span(COAST, NEAR_INLAND), E[3], v, core);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[3], v, core_arid);
            if is_low(v) {
                add(rows, t, h, COAST, E[4], v, beach);
                add(rows, t, h, span(NEAR_INLAND, FAR_INLAND), E[4], v, core);
            } else {
                add(rows, t, h, span(COAST, FAR_INLAND), E[4], v, core);
            }
            add(rows, t, h, COAST, E[5], v, windswept_coast);
            add(rows, t, h, NEAR_INLAND, E[5], v, windswept);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[5], v, hills);
            if is_low(v) {
                add(rows, t, h, COAST, E[6], v, beach);
            } else {
                add(rows, t, h, COAST, E[6], v, core);
            }
            if i == 0 {
                add(rows, t, h, span(NEAR_INLAND, FAR_INLAND), E[6], v, core);
            }
        }
    }
}

fn add_low_slice(rows: &mut Vec<Row>, v: AxisRange) {
    add(rows, FULL, FULL, COAST, span(E[0], E[2]), v, Beach);
    add(
        rows,
        UNFROZEN,
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Swamp,
    );

    for i in 0..5 {
        let t = T[i];
        for j in 0..5 {
            let h = H[j];
            let core = pick_core(i, j, v);
            let core_arid = pick_core_or_arid_if_hot(i, j, v);
            let core_arid_slope = pick_core_or_arid_if_hot_or_slope_if_cold(i, j, v);
            let beach = pick_beach(i);
            let windswept = maybe_windswept_open(i, j, v, core);
            let windswept_coast = pick_windswept_coast(i, j, v);
            add(rows, t, h, NEAR_INLAND, span(E[0], E[1]), v, core_arid);
            add(
                rows,
                t,
                h,
                span(MID_INLAND, FAR_INLAND),
                span(E[0], E[1]),
                v,
                core_arid_slope,
            );
            add(rows, t, h, NEAR_INLAND, span(E[2], E[3]), v, core);
            add(
                rows,
                t,
                h,
                span(MID_INLAND, FAR_INLAND),
                span(E[2], E[3]),
                v,
                core_arid,
            );
            add(rows, t, h, COAST, span(E[3], E[4]), v, beach);
            add(rows, t, h, span(NEAR_INLAND, FAR_INLAND), E[4], v, core);
            add(rows, t, h, COAST, E[5], v, windswept_coast);
            add(rows, t, h, NEAR_INLAND, E[5], v, windswept);
            add(rows, t, h, span(MID_INLAND, FAR_INLAND), E[5], v, core);
            add(rows, t, h, COAST, E[6], v, beach);
            if i == 0 {
                add(rows, t, h, span(NEAR_INLAND, FAR_INLAND), E[6], v, core);
            }
        }
    }
}

/// The centre variance band (variance ≈ 0): the reference river slice. Faithful
/// port of the reference `addValleys` — rivers across coast/inland by erosion
/// band, swamps at the wettest erosion shoulder, and the ordinary middle biome at
/// the driest mid/far-inland erosion (rivers do not cut the high-continentality,
/// low-erosion uplands). The reference's stony-shore branch applies only to a
/// wholly-negative-variance slice, so this centre slice (which straddles zero)
/// never takes it. Reference `river`/`frozen_river` both map to our single
/// `River`; `swamp`/`mangrove_swamp` both map to `Swamp`.
fn add_valley_slice(rows: &mut Vec<Row>, v: AxisRange) {
    // Rivers. Frozen + unfrozen both map to River, so the reference temperature
    // split collapses to FULL across these rows.
    add(rows, FULL, FULL, COAST, span(E[0], E[1]), v, River);
    add(rows, FULL, FULL, NEAR_INLAND, span(E[0], E[1]), v, River);
    add(
        rows,
        FULL,
        FULL,
        span(COAST, FAR_INLAND),
        span(E[2], E[5]),
        v,
        River,
    );
    add(rows, FULL, FULL, COAST, E[6], v, River);
    // Wettest erosion shoulder, inland: frozen → river, otherwise swamp.
    add(
        rows,
        T[0],
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        River,
    );
    add(
        rows,
        UNFROZEN,
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Swamp,
    );
    // Driest mid/far-inland erosion keeps the ordinary middle biome.
    for i in 0..5 {
        let t = T[i];
        for j in 0..5 {
            let h = H[j];
            let mid = pick_core_or_arid_if_hot(i, j, v);
            add(
                rows,
                t,
                h,
                span(MID_INLAND, FAR_INLAND),
                span(E[0], E[1]),
                v,
                mid,
            );
        }
    }
}

fn variance_slice(index: usize) -> AxisRange {
    AxisRange::new(VARIANCE_EDGES[index], VARIANCE_EDGES[index + 1])
}

/// The full `(rectangle, biome)` table for surface classification.
pub(crate) fn surface_biome_table() -> Vec<Row> {
    let mut rows = Vec::new();
    add_off_coast(&mut rows);

    // 13 variance slices, mirrored around zero: mid/high/peak/high/mid on the
    // low side, then low/valley/low across the centre, then mid/high/peak/high/mid
    // on the high side. The centre (valley) band carries the lowest relief and is
    // the reference's river slice (`add_valley_slice`).
    let dispatch: [fn(&mut Vec<Row>, AxisRange); 13] = [
        add_mid_slice,
        add_high_slice,
        add_peaks,
        add_high_slice,
        add_mid_slice,
        add_low_slice,
        add_valley_slice,
        add_low_slice,
        add_mid_slice,
        add_high_slice,
        add_peaks,
        add_high_slice,
        add_mid_slice,
    ];
    for (slice, build) in dispatch.into_iter().enumerate() {
        build(&mut rows, variance_slice(slice));
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rivers are assigned, and only in the centre variance band (the reference
    /// river slice). Classification only ever returns a row's biome, so pinning
    /// that River rows exist AND all sit in the centre variance slice guarantees
    /// rivers appear at variance ≈ 0 and never leak into other bands.
    #[test]
    fn rivers_are_assigned_only_in_the_centre_variance_band() {
        use super::super::climate::ClimateAxis;
        let centre = variance_slice(6);
        let river_rows: Vec<_> = surface_biome_table()
            .into_iter()
            .filter(|(_, biome)| *biome == Biome::River)
            .collect();
        assert!(!river_rows.is_empty(), "surface table must assign River");
        for (rect, _) in river_rows {
            let var = rect
                .axis_range(ClimateAxis::Variance)
                .expect("surface rect must expose variance");
            assert!(
                var.min >= centre.min && var.max <= centre.max,
                "River row variance [{}, {}] escaped the centre band [{}, {}]",
                var.min,
                var.max,
                centre.min,
                centre.max
            );
        }
    }
}
