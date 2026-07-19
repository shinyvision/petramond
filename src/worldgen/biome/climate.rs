//! Multi-axis climate classification for the staged surface worldgen rewrite.
//!
//! This module classifies a sampled climate vector from the density graph into
//! a final game-facing [`Biome`] without shaping terrain or placing blocks.

use crate::biome::Biome;
use crate::worldgen::density::terrain::channels;
use crate::worldgen::graph::{SamplePoint, ScalarGraph};

const SURFACE_AXIS_COUNT: usize = 5;

pub(crate) const CLIMATE_SAMPLE_CELL_X: i32 = 4;
pub(crate) const CLIMATE_SAMPLE_CELL_Y: i32 = 4;
pub(crate) const CLIMATE_SAMPLE_CELL_Z: i32 = 4;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClimateAxis {
    /// Read by the sea-ice pass (`density::surface::waterline_block`), besides
    /// the classifier's rectangle machinery.
    Temperature,
    #[cfg(test)]
    Humidity,
    Continentality,
    #[cfg(test)]
    Erosion,
    #[cfg(test)]
    Variance,
}

impl ClimateAxis {
    const fn index(self) -> usize {
        match self {
            Self::Temperature => 0,
            #[cfg(test)]
            Self::Humidity => 1,
            Self::Continentality => 2,
            #[cfg(test)]
            Self::Erosion => 3,
            #[cfg(test)]
            Self::Variance => 4,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct AxisRange {
    pub min: f32,
    pub max: f32,
}

impl AxisRange {
    pub(crate) const fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    pub(crate) fn distance_squared(self, value: f32) -> f64 {
        let lo = self.min.min(self.max);
        let hi = self.min.max(self.max);
        if value < lo {
            squared(f64::from(lo - value))
        } else if value > hi {
            squared(f64::from(value - hi))
        } else {
            0.0
        }
    }

}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SurfaceClimate {
    axes: [f32; SURFACE_AXIS_COUNT],
}

impl SurfaceClimate {
    pub(crate) const fn new(
        temperature: f32,
        humidity: f32,
        continentality: f32,
        erosion: f32,
        variance: f32,
    ) -> Self {
        Self {
            axes: [temperature, humidity, continentality, erosion, variance],
        }
    }

    pub(crate) fn from_graph(graph: &ScalarGraph, point: SamplePoint) -> Option<Self> {
        Some(Self::new(
            graph.evaluate_channel(channels::TEMPERATURE, point)? as f32,
            graph.evaluate_channel(channels::HUMIDITY, point)? as f32,
            graph.evaluate_channel(channels::CONTINENTALITY, point)? as f32,
            graph.evaluate_channel(channels::EROSION, point)? as f32,
            graph.evaluate_channel(channels::VARIANCE, point)? as f32,
        ))
    }

    /// Bilinear blend of four corner climates (`fx`/`fz` in `0..1` from the
    /// `00` corner). Used to smooth per-4×4-cell climate samples up to per-column
    /// resolution — valid because climate is low-frequency, so a 4-block span is
    /// near-linear.
    pub(crate) fn bilerp(c00: Self, c10: Self, c01: Self, c11: Self, fx: f32, fz: f32) -> Self {
        let mut axes = [0.0f32; SURFACE_AXIS_COUNT];
        for (i, axis) in axes.iter_mut().enumerate() {
            let low = c00.axes[i] + (c10.axes[i] - c00.axes[i]) * fx;
            let high = c01.axes[i] + (c11.axes[i] - c01.axes[i]) * fx;
            *axis = low + (high - low) * fz;
        }
        Self { axes }
    }

    pub(crate) const fn get(self, axis: ClimateAxis) -> Option<f32> {
        Some(self.axes[axis.index()])
    }

    const fn axes(self) -> [f32; SURFACE_AXIS_COUNT] {
        self.axes
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ClimateRect {
    axes: [AxisRange; SURFACE_AXIS_COUNT],
    /// A flat additive fitness penalty (added as `offset²`) used to bias ties
    /// toward the intended biome. Defaults to 0; rarely nonzero.
    offset: f32,
}

impl ClimateRect {
    pub(crate) const fn surface(
        temperature: AxisRange,
        humidity: AxisRange,
        continentality: AxisRange,
        erosion: AxisRange,
        variance: AxisRange,
    ) -> Self {
        Self {
            axes: [temperature, humidity, continentality, erosion, variance],
            offset: 0.0,
        }
    }

    #[cfg(test)]
    pub(crate) const fn with_offset(mut self, offset: f32) -> Self {
        self.offset = offset;
        self
    }

    #[cfg(test)]
    pub(crate) fn axis_range(self, axis: ClimateAxis) -> Option<AxisRange> {
        Some(self.axes[axis.index()])
    }

    pub(crate) fn distance_squared(self, climate: SurfaceClimate) -> f64 {
        let values = climate.axes();
        let surface_distance = self
            .axes
            .into_iter()
            .zip(values)
            .map(|(range, value)| range.distance_squared(value))
            .sum::<f64>();
        let offset_distance = f64::from(self.offset) * f64::from(self.offset);
        surface_distance + offset_distance
    }

}

#[derive(Copy, Clone, Debug)]
#[cfg(test)]
pub(crate) struct BiomeClimateEntry<'a> {
    pub biome: Biome,
    pub rectangles: &'a [ClimateRect],
}

/// Bin count for the per-axis containment buckets (power of two; the axes are
/// normalized to roughly `[-1, 1]`, and the end bins absorb out-of-range values).
const AXIS_BIN_COUNT: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct BiomeClimateIndex {
    rects: Vec<IndexedRect>,
    /// Ordered row indices whose VARIANCE range intersects each bin of the
    /// normalized `[-1, 1]` variance axis. A containing rect must contain the
    /// query's variance value, so it provably appears in the query's bin — the
    /// containment scan walks only that bin's rows (in table order) instead of
    /// the whole table. The surface table is organized as variance slices, so
    /// this axis discriminates best.
    variance_bins: Vec<Vec<u32>>,
}

/// The variance-axis bin holding `value` (end bins absorb out-of-range).
fn axis_bin(value: f32) -> usize {
    let t = (f64::from(value) + 1.0) / 2.0 * AXIS_BIN_COUNT as f64;
    (t.floor().max(0.0) as usize).min(AXIS_BIN_COUNT - 1)
}

impl BiomeClimateIndex {
    #[cfg(test)]
    pub(crate) fn new(entries: &[BiomeClimateEntry<'_>]) -> Self {
        let rects = entries
            .iter()
            .enumerate()
            .flat_map(|(entry_order, entry)| {
                entry
                    .rectangles
                    .iter()
                    .copied()
                    .map(move |rect| (entry_order, entry.biome, rect))
            })
            .enumerate()
            .map(|(order, (entry_order, biome, rect))| IndexedRect {
                order,
                entry_order,
                biome,
                rect,
            })
            .collect::<Vec<_>>();
        Self::from_indexed(rects)
    }

    /// Build from a flat, ordered list of `(rectangle, biome)` rows. Row order is
    /// the only tiebreak between equal-fitness rectangles, so the caller's ordering
    /// is preserved verbatim (unlike [`Self::new`], which groups by biome first).
    pub(crate) fn from_rects(rows: &[(ClimateRect, Biome)]) -> Self {
        let rects = rows
            .iter()
            .enumerate()
            .map(|(order, &(rect, biome))| IndexedRect {
                order,
                entry_order: order,
                biome,
                rect,
            })
            .collect::<Vec<_>>();
        Self::from_indexed(rects)
    }

    fn from_indexed(rects: Vec<IndexedRect>) -> Self {
        let variance_axis = SURFACE_AXIS_COUNT - 1;
        let mut variance_bins = vec![Vec::new(); AXIS_BIN_COUNT];
        for (i, rect) in rects.iter().enumerate() {
            let range = rect.rect.axes[variance_axis];
            let (lo, hi) = (range.min.min(range.max), range.min.max(range.max));
            // End bins are unbounded: a range touching an edge value lands in
            // the same bin any out-of-range query value quantizes to.
            for (bin, rows) in variance_bins.iter_mut().enumerate() {
                let bin_lo = -1.0 + bin as f32 * (2.0 / AXIS_BIN_COUNT as f32);
                let bin_hi = bin_lo + 2.0 / AXIS_BIN_COUNT as f32;
                let bin_lo = if bin == 0 { f32::NEG_INFINITY } else { bin_lo };
                let bin_hi = if bin == AXIS_BIN_COUNT - 1 {
                    f32::INFINITY
                } else {
                    bin_hi
                };
                if lo < bin_hi && hi >= bin_lo {
                    rows.push(i as u32);
                }
            }
        }
        Self {
            rects,
            variance_bins,
        }
    }

    pub(crate) fn default_surface() -> &'static Self {
        static DEFAULT: std::sync::LazyLock<BiomeClimateIndex> = std::sync::LazyLock::new(|| {
            BiomeClimateIndex::from_rects(&super::surface_table::surface_biome_table())
        });
        &DEFAULT
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Nearest-rect classification: lowest fitness distance, ties broken by
    /// stable `(entry_order, order)` — i.e. table row order.
    ///
    /// The scan runs the rows IN ORDER, so the first exact containment
    /// (`distance == 0`) is the final winner and returns immediately: a later
    /// rect can at best tie at 0, and ties prefer the earlier row. The
    /// surface table covers the whole climate space, so nearly every query
    /// takes this early exit — a KD-tree over the rows was measurably SLOWER
    /// here, because the heavily overlapping rows give most nodes
    /// distance-0 bounds (no pruning) and the spatial visit order defeats
    /// the order-tiebreak early exit. Byte-parity with the exhaustive scan
    /// is pinned by `index_matches_bruteforce_for_surface_queries`.
    pub(crate) fn classify_surface(&self, climate: SurfaceClimate) -> Option<Biome> {
        // Containment pass over the query's variance bin only: any rect
        // containing the query contains its variance value, so it is in this
        // bin's list; the list preserves table order, making the first
        // distance-0 hit the global tie-break winner.
        let bin = &self.variance_bins[axis_bin(climate.axes[SURFACE_AXIS_COUNT - 1])];
        for &i in bin {
            let rect = &self.rects[i as usize];
            if rect.rect.distance_squared(climate) == 0.0 {
                return Some(rect.biome);
            }
        }
        // No containing rect anywhere: exhaustive nearest scan (rare — the
        // surface table covers the climate space nearly everywhere).
        let mut best = Candidate::none();
        for rect in &self.rects {
            best.consider(rect, rect.rect.distance_squared(climate));
        }
        best.biome
    }

    #[cfg(test)]
    pub(crate) fn classify_surface_bruteforce(&self, climate: SurfaceClimate) -> Option<Biome> {
        let mut best = Candidate::none();
        for rect in &self.rects {
            best.consider(rect, rect.rect.distance_squared(climate));
        }
        best.biome
    }
}

#[derive(Copy, Clone, Debug)]
struct IndexedRect {
    order: usize,
    entry_order: usize,
    biome: Biome,
    rect: ClimateRect,
}

#[derive(Copy, Clone, Debug)]
struct Candidate {
    distance: f64,
    order: usize,
    entry_order: usize,
    biome: Option<Biome>,
}

impl Candidate {
    fn none() -> Self {
        Self {
            distance: f64::INFINITY,
            order: usize::MAX,
            entry_order: usize::MAX,
            biome: None,
        }
    }

    // The `offset` penalty lives inside `distance` (added as offset²), so the
    // fitness total already encodes any biome bias; equal totals break by stable
    // insertion order only.
    fn consider(&mut self, rect: &IndexedRect, distance: f64) {
        if distance < self.distance
            || (distance == self.distance
                && (rect.entry_order, rect.order) < (self.entry_order, self.order))
        {
            self.distance = distance;
            self.order = rect.order;
            self.entry_order = rect.entry_order;
            self.biome = Some(rect.biome);
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ClimateSampleCell {
    x: i32,
    y: i32,
    z: i32,
}

impl ClimateSampleCell {
    /// Raw cell coordinates, for world-anchored memo keys.
    pub(crate) fn coords(self) -> (i32, i32, i32) {
        (self.x, self.y, self.z)
    }

    pub(crate) fn surface(wx: i32, wz: i32) -> Self {
        Self {
            x: wx.div_euclid(CLIMATE_SAMPLE_CELL_X),
            y: 0,
            z: wz.div_euclid(CLIMATE_SAMPLE_CELL_Z),
        }
    }

    /// A surface cell from its grid indices directly (the inverse of dividing a
    /// world coordinate by the cell size). Used to address interpolation corners.
    pub(crate) const fn at_surface_indices(x: i32, z: i32) -> Self {
        Self { x, y: 0, z }
    }

    pub(crate) fn origin(self) -> (i32, i32, i32) {
        (
            self.x * CLIMATE_SAMPLE_CELL_X,
            self.y * CLIMATE_SAMPLE_CELL_Y,
            self.z * CLIMATE_SAMPLE_CELL_Z,
        )
    }

    fn sample_point(self) -> SamplePoint {
        let (x, y, z) = self.origin();
        SamplePoint::new(f64::from(x), f64::from(y), f64::from(z))
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ClimateSample {
    pub cell: ClimateSampleCell,
    pub climate: SurfaceClimate,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ClimateSampler<'a> {
    graph: &'a ScalarGraph,
}

impl<'a> ClimateSampler<'a> {
    pub(crate) fn new(graph: &'a ScalarGraph) -> Self {
        Self { graph }
    }

    pub(crate) fn sample_surface_cell(self, cell: ClimateSampleCell) -> Option<ClimateSample> {
        let climate = SurfaceClimate::from_graph(self.graph, cell.sample_point())?;
        Some(ClimateSample { cell, climate })
    }
}





fn squared(value: f64) -> f64 {
    value * value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::graph::{Axis, Channel};

    const ANY: AxisRange = AxisRange::new(-1.0, 1.0);

    const fn test_rect(min: f32, max: f32) -> ClimateRect {
        ClimateRect::surface(
            AxisRange::new(min, max),
            AxisRange::new(min, max),
            AxisRange::new(min, max),
            AxisRange::new(min, max),
            AxisRange::new(min, max),
        )
    }

    #[test]
    fn axis_range_distance_is_zero_inside_and_squared_outside() {
        let range = AxisRange::new(0.25, 0.75);

        assert_eq!(range.distance_squared(0.25), 0.0);
        assert_eq!(range.distance_squared(0.50), 0.0);
        assert_eq!(range.distance_squared(0.75), 0.0);
        assert!((range.distance_squared(0.10) - 0.0225).abs() < 1.0e-6);
        assert!((range.distance_squared(0.95) - 0.04).abs() < 1.0e-6);
    }

    #[test]
    fn rectangle_distance_is_zero_when_all_surface_axes_are_inside() {
        let rect = ClimateRect::surface(
            AxisRange::new(0.0, 0.4),
            AxisRange::new(0.1, 0.5),
            AxisRange::new(0.2, 0.6),
            AxisRange::new(0.3, 0.7),
            AxisRange::new(0.4, 0.8),
        );
        let climate = SurfaceClimate::new(0.2, 0.3, 0.4, 0.5, 0.6);

        assert_eq!(rect.distance_squared(climate), 0.0);
    }

    #[test]
    fn nearest_rectangle_uses_squared_distance_to_closest_bounds() {
        static COLD: &[ClimateRect] = &[ClimateRect::surface(
            AxisRange::new(0.0, 0.2),
            ANY,
            ANY,
            ANY,
            ANY,
        )];
        static WARM: &[ClimateRect] = &[ClimateRect::surface(
            AxisRange::new(0.6, 0.8),
            ANY,
            ANY,
            ANY,
            ANY,
        )];
        let index = BiomeClimateIndex::new(&[
            BiomeClimateEntry {
                biome: Biome::SnowyTundra,
                rectangles: COLD,
            },
            BiomeClimateEntry {
                biome: Biome::Desert,
                rectangles: WARM,
            },
        ]);

        assert_eq!(
            index.classify_surface(SurfaceClimate::new(0.50, 0.0, 0.0, 0.0, 0.0)),
            Some(Biome::Desert)
        );
    }

    #[test]
    fn index_matches_bruteforce_for_surface_queries() {
        let index = BiomeClimateIndex::default_surface();

        for temperature in [-0.95, -0.62, -0.26, 0.22, 0.76] {
            for humidity in [-0.90, -0.34, 0.16, 0.82] {
                for continentality in [-0.98, -0.44, -0.12, 0.46, 0.96] {
                    for erosion in [-0.92, -0.28, 0.02, 0.54, 1.0] {
                        for variance in [-1.0, -0.38, 0.0, 0.34, 0.86] {
                            let climate = SurfaceClimate::new(
                                temperature,
                                humidity,
                                continentality,
                                erosion,
                                variance,
                            );
                            assert_eq!(
                                index.classify_surface(climate),
                                index.classify_surface_bruteforce(climate)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn default_index_classifies_representative_signed_climates() {
        let index = BiomeClimateIndex::default_surface();

        assert!(!index.is_empty());
        for climate in [
            SurfaceClimate::new(-0.8, -0.1, -0.9, 0.2, -0.3),
            SurfaceClimate::new(-0.7, 0.7, 0.2, 0.4, 0.1),
            SurfaceClimate::new(0.75, -0.75, 0.4, 0.6, -0.2),
            SurfaceClimate::new(0.2, 0.5, 0.7, -0.5, 0.8),
        ] {
            assert!(index.classify_surface(climate).is_some());
        }
    }

    #[test]
    fn non_empty_index_always_returns_a_biome() {
        const RECTANGLES: &[ClimateRect] = &[test_rect(0.25, 0.75)];
        let index = BiomeClimateIndex::new(&[BiomeClimateEntry {
            biome: Biome::Plains,
            rectangles: RECTANGLES,
        }]);

        assert_eq!(
            index.classify_surface(SurfaceClimate::new(99.0, -50.0, 7.0, 4.0, 2.0)),
            Some(Biome::Plains)
        );
    }

    #[test]
    fn offset_penalty_breaks_ties_toward_the_unpenalized_biome() {
        const BROAD: &[ClimateRect] =
            &[ClimateRect::surface(ANY, ANY, ANY, ANY, ANY).with_offset(0.2)];
        const SPECIFIC: &[ClimateRect] = &[ClimateRect::surface(
            AxisRange::new(-0.20, 0.20),
            AxisRange::new(-0.20, 0.20),
            AxisRange::new(-0.20, 0.20),
            AxisRange::new(-0.20, 0.20),
            AxisRange::new(-0.20, 0.20),
        )];
        let index = BiomeClimateIndex::new(&[
            BiomeClimateEntry {
                biome: Biome::Plains,
                rectangles: BROAD,
            },
            BiomeClimateEntry {
                biome: Biome::Meadow,
                rectangles: SPECIFIC,
            },
        ]);
        let climate = SurfaceClimate::new(0.0, 0.0, 0.0, 0.0, 0.0);

        // Both rectangles contain the sample (zero range-distance), but BROAD carries
        // an `offset` penalty, so the unpenalized biome wins.
        assert_eq!(index.classify_surface(climate), Some(Biome::Meadow));
        assert_eq!(
            index.classify_surface_bruteforce(climate),
            Some(Biome::Meadow)
        );
    }

    #[test]
    fn surface_climate_from_graph_reads_five_channels() {
        let mut graph = ScalarGraph::new();
        let temperature = graph.constant(-0.25);
        let humidity = graph.constant(0.25);
        let continentality = graph.constant(-0.75);
        let erosion = graph.constant(0.75);
        let variance = graph.constant(0.5);
        graph.set_channel(Channel::new(channels::TEMPERATURE), temperature);
        graph.set_channel(Channel::new(channels::HUMIDITY), humidity);
        graph.set_channel(Channel::new(channels::CONTINENTALITY), continentality);
        graph.set_channel(Channel::new(channels::EROSION), erosion);
        graph.set_channel(Channel::new(channels::VARIANCE), variance);

        let climate = SurfaceClimate::from_graph(&graph, SamplePoint::new(0.0, 0.0, 0.0))
            .expect("five climate channels should classify");

        assert_eq!(climate.get(ClimateAxis::Temperature), Some(-0.25));
        assert_eq!(climate.get(ClimateAxis::Humidity), Some(0.25));
        assert_eq!(climate.get(ClimateAxis::Continentality), Some(-0.75));
        assert_eq!(climate.get(ClimateAxis::Erosion), Some(0.75));
        assert_eq!(climate.get(ClimateAxis::Variance), Some(0.5));
    }

    #[test]
    fn cell_sampling_uses_world_anchored_euclidean_cells() {
        let mut graph = ScalarGraph::new();
        let x = graph.axis(Axis::X);
        let y = graph.axis(Axis::Y);
        let z = graph.axis(Axis::Z);
        let erosion = graph.constant(0.5);
        let variance = graph.constant(0.25);
        graph.set_channel(Channel::new(channels::TEMPERATURE), x);
        graph.set_channel(Channel::new(channels::HUMIDITY), z);
        graph.set_channel(Channel::new(channels::CONTINENTALITY), y);
        graph.set_channel(Channel::new(channels::EROSION), erosion);
        graph.set_channel(Channel::new(channels::VARIANCE), variance);

        let cell = ClimateSampleCell::surface(-1, -5);
        assert_eq!(cell.origin(), (-4, 0, -8));
        assert_eq!(ClimateSampleCell::surface(0, 3).origin(), (0, 0, 0));

        let sample = ClimateSampler::new(&graph)
            .sample_surface_cell(cell)
            .unwrap();
        assert_eq!(sample.cell, cell);
        assert_eq!(sample.climate.get(ClimateAxis::Temperature), Some(-4.0));
        assert_eq!(sample.climate.get(ClimateAxis::Humidity), Some(-8.0));
        assert_eq!(sample.climate.get(ClimateAxis::Continentality), Some(0.0));
        assert_eq!(sample.climate.get(ClimateAxis::Variance), Some(0.25));
    }
}
