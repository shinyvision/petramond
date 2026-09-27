//! Builds the surface biome table. The live game loads `assets/climate_table.json` instead (see
//! [`crate::data::climate_table`]); this code is the test reference that file was generated from,
//! and the file has to match its rows exactly and in order.
//!
//! It follows the reference generator: base biomes on a temperature/humidity grid, sliced by
//! erosion band and mirrored across the variance fold, with coast, ocean and peak cases on top.

use petramond_world::biome::Biome;

use super::climate::{AxisRange, ClimateRect};

type Row = (ClimateRect, Biome);

const FULL: AxisRange = AxisRange::new(-1.0, 1.0);

pub const FROZEN_TEMPERATURE_MAX: f32 = -0.3;

const T: [AxisRange; 5] = [
    AxisRange::new(-1.0, FROZEN_TEMPERATURE_MAX),
    AxisRange::new(FROZEN_TEMPERATURE_MAX, -0.15),
    AxisRange::new(-0.15, 0.2),
    AxisRange::new(0.2, 0.55),
    AxisRange::new(0.55, 1.0),
];

const H: [AxisRange; 5] = [
    AxisRange::new(-1.0, -0.35),
    AxisRange::new(-0.35, -0.1),
    AxisRange::new(-0.1, 0.1),
    AxisRange::new(0.1, 0.3),
    AxisRange::new(0.3, 1.0),
];

const E: [AxisRange; 7] = [
    AxisRange::new(-1.0, -0.78),
    AxisRange::new(-0.78, -0.375),
    AxisRange::new(-0.375, -0.2225),
    AxisRange::new(-0.2225, 0.05),
    AxisRange::new(0.05, 0.45),
    AxisRange::new(0.45, 0.55),
    AxisRange::new(0.55, 1.0),
];

const MUSHROOM: AxisRange = AxisRange::new(-1.2, -1.05);
const DEEP_OCEAN_C: AxisRange = AxisRange::new(-1.05, -0.455);
const OCEAN_C: AxisRange = AxisRange::new(-0.455, -0.19);
const COAST: AxisRange = AxisRange::new(-0.19, -0.11);
const NEAR_INLAND: AxisRange = AxisRange::new(-0.11, 0.03);
const MID_INLAND: AxisRange = AxisRange::new(0.03, 0.3);
const FAR_INLAND: AxisRange = AxisRange::new(0.3, 1.0);

const UNFROZEN: AxisRange = span(T[1], T[4]);

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

const fn span(a: AxisRange, b: AxisRange) -> AxisRange {
    AxisRange::new(a.min, b.max)
}

fn is_low(variance: AxisRange) -> bool {
    variance.max < 0.0
}

const CORE: [[Biome; 5]; 5] = [
    [
        Biome::SNOWY_PLAINS,
        Biome::SNOWY_PLAINS,
        Biome::SNOWY_TUNDRA,
        Biome::SNOWY_TAIGA,
        Biome::TAIGA,
    ],
    [
        Biome::PLAINS,
        Biome::PLAINS,
        Biome::FOREST,
        Biome::TAIGA,
        Biome::OLD_GROWTH_TAIGA,
    ],
    [
        Biome::FOREST,
        Biome::PLAINS,
        Biome::FOREST,
        Biome::FOREST,
        Biome::FOREST,
    ],
    [
        Biome::SAVANNA,
        Biome::SAVANNA,
        Biome::FOREST,
        Biome::FOREST,
        Biome::FOREST,
    ],
    [
        Biome::DESERT,
        Biome::DESERT,
        Biome::DESERT,
        Biome::DESERT,
        Biome::DESERT,
    ],
];

const CORE_HIGH: [[Option<Biome>; 5]; 5] = [
    [
        Some(Biome::SNOWY_TUNDRA),
        None,
        Some(Biome::SNOWY_TAIGA),
        None,
        None,
    ],
    [None, None, None, None, Some(Biome::REDWOOD_FOREST)],
    [Some(Biome::PLAINS), None, None, Some(Biome::FOREST), None],
    [
        None,
        None,
        Some(Biome::PLAINS),
        Some(Biome::FOREST),
        Some(Biome::FOREST),
    ],
    [None, None, None, None, None],
];

const PLATEAU: [[Biome; 5]; 5] = [
    [
        Biome::SNOWY_TUNDRA,
        Biome::SNOWY_TUNDRA,
        Biome::SNOWY_TUNDRA,
        Biome::SNOWY_TAIGA,
        Biome::SNOWY_TAIGA,
    ],
    [
        Biome::MEADOW,
        Biome::MEADOW,
        Biome::FOREST,
        Biome::TAIGA,
        Biome::OLD_GROWTH_TAIGA,
    ],
    [
        Biome::MEADOW,
        Biome::MEADOW,
        Biome::MEADOW,
        Biome::MEADOW,
        Biome::FOREST,
    ],
    [
        Biome::SAVANNA,
        Biome::SAVANNA,
        Biome::FOREST,
        Biome::FOREST,
        Biome::FOREST,
    ],
    [
        Biome::DESERT,
        Biome::DESERT,
        Biome::DESERT,
        Biome::DESERT,
        Biome::DESERT,
    ],
];

const PLATEAU_HIGH: [[Option<Biome>; 5]; 5] = [
    [Some(Biome::SNOWY_TUNDRA), None, None, None, None],
    [
        None,
        None,
        Some(Biome::MEADOW),
        Some(Biome::MEADOW),
        Some(Biome::REDWOOD_FOREST),
    ],
    [None, None, Some(Biome::FOREST), Some(Biome::FOREST), None],
    [None, None, None, None, None],
    [Some(Biome::DESERT), Some(Biome::DESERT), None, None, None],
];

const HILLS: [[Option<Biome>; 5]; 5] = [
    [Some(Biome::WINDSWEPT_HILLS); 5],
    [Some(Biome::WINDSWEPT_HILLS); 5],
    [Some(Biome::WINDSWEPT_HILLS); 5],
    [None; 5],
    [None; 5],
];

const OCEANS: [[Biome; 5]; 2] = [
    [
        Biome::DEEP_OCEAN,
        Biome::DEEP_OCEAN,
        Biome::DEEP_OCEAN,
        Biome::DEEP_OCEAN,
        Biome::OCEAN,
    ],
    [
        Biome::OCEAN,
        Biome::OCEAN,
        Biome::OCEAN,
        Biome::OCEAN,
        Biome::OCEAN,
    ],
];

fn pick_core(i: usize, j: usize, v: AxisRange) -> Biome {
    if is_low(v) {
        CORE[i][j]
    } else {
        CORE_HIGH[i][j].unwrap_or(CORE[i][j])
    }
}

fn pick_core_or_arid_if_hot(i: usize, j: usize, v: AxisRange) -> Biome {
    if i == 4 {
        Biome::DESERT
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
        Biome::SAVANNA
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
        Biome::DESERT
    } else {
        Biome::BEACH
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
        Biome::SNOWY_PEAKS
    } else if i == 3 {
        Biome::STONY_PEAKS
    } else {
        Biome::DESERT
    }
}

fn pick_slope(i: usize, j: usize, v: AxisRange) -> Biome {
    if i >= 3 {
        pick_plateau(i, j, v)
    } else if j <= 1 {
        Biome::SNOWY_SLOPES
    } else {
        Biome::GROVE
    }
}

fn pick_hills(i: usize, j: usize, v: AxisRange) -> Biome {
    HILLS[i][j].unwrap_or_else(|| pick_core(i, j, v))
}

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
    add(rows, FULL, FULL, MUSHROOM, FULL, FULL, Biome::PLAINS);
    for (i, &t) in T.iter().enumerate() {
        add(rows, t, FULL, DEEP_OCEAN_C, FULL, FULL, OCEANS[0][i]);
        add(rows, t, FULL, OCEAN_C, FULL, FULL, OCEANS[1][i]);
    }
}

fn add_peaks(rows: &mut Vec<Row>, v: AxisRange) {
    for (i, &t) in T.iter().enumerate() {
        for (j, &h) in H.iter().enumerate() {
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
    for (i, &t) in T.iter().enumerate() {
        for (j, &h) in H.iter().enumerate() {
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
    add(rows, FULL, FULL, COAST, span(E[0], E[2]), v, Biome::BEACH);
    add(
        rows,
        UNFROZEN,
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Biome::SWAMP,
    );

    for (i, &t) in T.iter().enumerate() {
        for (j, &h) in H.iter().enumerate() {
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
    add(rows, FULL, FULL, COAST, span(E[0], E[2]), v, Biome::BEACH);
    add(
        rows,
        UNFROZEN,
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Biome::SWAMP,
    );

    for (i, &t) in T.iter().enumerate() {
        for (j, &h) in H.iter().enumerate() {
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

/// Variance ≈ 0, where the reference puts its rivers. Ported from `addValleys`, so rivers follow
/// erosion bands, swamps take the wettest shoulder, and the driest inland erosion keeps the normal
/// middle biome. Stony shores never show up here since this slice straddles zero.
fn add_valley_slice(rows: &mut Vec<Row>, v: AxisRange) {
    add(rows, FULL, FULL, COAST, span(E[0], E[1]), v, Biome::RIVER);
    add(
        rows,
        FULL,
        FULL,
        NEAR_INLAND,
        span(E[0], E[1]),
        v,
        Biome::RIVER,
    );
    add(
        rows,
        FULL,
        FULL,
        span(COAST, FAR_INLAND),
        span(E[2], E[5]),
        v,
        Biome::RIVER,
    );
    add(rows, FULL, FULL, COAST, E[6], v, Biome::RIVER);
    add(
        rows,
        T[0],
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Biome::RIVER,
    );
    add(
        rows,
        UNFROZEN,
        FULL,
        span(NEAR_INLAND, FAR_INLAND),
        E[6],
        v,
        Biome::SWAMP,
    );
    for (i, &t) in T.iter().enumerate() {
        for (j, &h) in H.iter().enumerate() {
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

pub fn surface_biome_table() -> Vec<Row> {
    let mut rows = Vec::new();
    add_off_coast(&mut rows);

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

    #[test]
    fn rivers_are_assigned_only_in_the_centre_variance_band() {
        use super::super::climate::ClimateAxis;
        let centre = variance_slice(6);
        let river_rows: Vec<_> = surface_biome_table()
            .into_iter()
            .filter(|(_, biome)| *biome == Biome::RIVER)
            .collect();
        assert!(
            !river_rows.is_empty(),
            "surface table must assign Biome::RIVER"
        );
        for (rect, _) in river_rows {
            let var = rect
                .axis_range(ClimateAxis::Variance)
                .expect("surface rect must expose variance");
            assert!(
                var.min >= centre.min && var.max <= centre.max,
                "Biome::RIVER row variance [{}, {}] escaped the centre band [{}, {}]",
                var.min,
                var.max,
                centre.min,
                centre.max
            );
        }
    }
}
