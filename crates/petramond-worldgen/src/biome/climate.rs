use crate::density::terrain::channels;
use crate::graph::{SamplePoint, ScalarGraph};
use petramond_world::biome::Biome;

const SURFACE_AXIS_COUNT: usize = 5;

pub const CLIMATE_SAMPLE_CELL_X: i32 = 4;
pub const CLIMATE_SAMPLE_CELL_Y: i32 = 4;
pub const CLIMATE_SAMPLE_CELL_Z: i32 = 4;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ClimateAxis {
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
pub struct AxisRange {
    pub min: f32,
    pub max: f32,
}

impl AxisRange {
    pub const fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    pub fn distance_squared(self, value: f32) -> f64 {
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
pub struct SurfaceClimate {
    axes: [f32; SURFACE_AXIS_COUNT],
}

impl SurfaceClimate {
    pub const fn new(
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

    /// The surface axes from a column's channel values (see
    /// [`crate::density::columns::ColumnClimate`]).
    pub(crate) fn from_column(column: &[f64; 6]) -> Self {
        Self {
            axes: std::array::from_fn(|axis| column[axis] as f32),
        }
    }

    pub fn from_graph(graph: &ScalarGraph, point: SamplePoint) -> Option<Self> {
        let nodes = [
            graph.channel_node(channels::TEMPERATURE)?,
            graph.channel_node(channels::HUMIDITY)?,
            graph.channel_node(channels::CONTINENTALITY)?,
            graph.channel_node(channels::EROSION)?,
            graph.channel_node(channels::VARIANCE)?,
        ];
        Some(Self {
            axes: graph
                .evaluate_nodes_cached(nodes, point, &mut graph.evaluation_cache())
                .map(|v| v as f32),
        })
    }

    pub fn bilerp(c00: Self, c10: Self, c01: Self, c11: Self, fx: f32, fz: f32) -> Self {
        let mut axes = [0.0f32; SURFACE_AXIS_COUNT];
        for (i, axis) in axes.iter_mut().enumerate() {
            let low = c00.axes[i] + (c10.axes[i] - c00.axes[i]) * fx;
            let high = c01.axes[i] + (c11.axes[i] - c01.axes[i]) * fx;
            *axis = low + (high - low) * fz;
        }
        Self { axes }
    }

    pub const fn get(self, axis: ClimateAxis) -> Option<f32> {
        Some(self.axes[axis.index()])
    }

    const fn axes(self) -> [f32; SURFACE_AXIS_COUNT] {
        self.axes
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClimateRect {
    axes: [AxisRange; SURFACE_AXIS_COUNT],
    offset: f32,
}

impl ClimateRect {
    pub const fn surface(
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

    pub const fn with_offset(mut self, offset: f32) -> Self {
        self.offset = offset;
        self
    }

    pub const fn axis_ranges(self) -> [AxisRange; SURFACE_AXIS_COUNT] {
        self.axes
    }

    pub const fn offset(self) -> f32 {
        self.offset
    }

    #[cfg(test)]
    pub fn axis_range(self, axis: ClimateAxis) -> Option<AxisRange> {
        Some(self.axes[axis.index()])
    }

    /// [`Self::distance_squared`], or `None` once the running sum passes
    /// `limit` (terms are non-negative, so the total could only be larger).
    #[inline]
    fn distance_squared_within(self, climate: SurfaceClimate, limit: f64) -> Option<f64> {
        let mut sum = -0.0;
        for (range, value) in self.axes.iter().zip(climate.axes) {
            sum += range.distance_squared(value);
            if sum > limit {
                return None;
            }
        }
        Some(sum + f64::from(self.offset) * f64::from(self.offset))
    }

    pub fn distance_squared(self, climate: SurfaceClimate) -> f64 {
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
pub struct BiomeClimateEntry<'a> {
    pub biome: Biome,
    pub rectangles: &'a [ClimateRect],
}

const AXIS_BIN_COUNT: usize = 64;

#[derive(Clone, Debug)]
pub struct BiomeClimateIndex {
    rects: Vec<IndexedRect>,
    containment: Vec<Containment>,
    /// Row indices per bin of the normalized `[-1, 1]` variance axis - a rect matching the query
    /// always sits in the query's bin, so we skip scanning the rest. Table is split on variance
    /// since that axis discriminates best.
    variance_bins: Vec<Vec<u32>>,
}

fn axis_bin(value: f32) -> usize {
    let t = (f64::from(value) + 1.0) / 2.0 * AXIS_BIN_COUNT as f64;
    (t.floor().max(0.0) as usize).min(AXIS_BIN_COUNT - 1)
}

impl BiomeClimateIndex {
    #[cfg(test)]
    pub fn new(entries: &[BiomeClimateEntry<'_>]) -> Self {
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

    pub fn from_rects(rows: &[(ClimateRect, Biome)]) -> Self {
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
        let containment = rects.iter().map(|r| Containment::of(r.rect)).collect();
        Self {
            rects,
            containment,
            variance_bins,
        }
    }

    pub fn default_surface() -> &'static Self {
        &crate::data::climate_table::table().index
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn classify_surface(&self, climate: SurfaceClimate) -> Option<Biome> {
        let bin = &self.variance_bins[axis_bin(climate.axes[SURFACE_AXIS_COUNT - 1])];
        for &i in bin {
            if self.containment[i as usize].contains(&climate.axes) {
                return Some(self.rects[i as usize].biome);
            }
        }
        // The query's bin usually holds the nearest rect; with it as the bar,
        // most rows stop after an axis or two.
        let mut best = Candidate::none();
        for i in bin.iter().copied().chain(0..self.rects.len() as u32) {
            let rect = &self.rects[i as usize];
            if let Some(distance) = rect.rect.distance_squared_within(climate, best.distance) {
                best.consider(rect, distance);
            }
        }
        best.biome
    }

    #[cfg(test)]
    pub fn classify_surface_bruteforce(&self, climate: SurfaceClimate) -> Option<Biome> {
        let mut best = Candidate::none();
        for rect in &self.rects {
            best.consider(rect, rect.rect.distance_squared(climate));
        }
        best.biome
    }
}

#[derive(Copy, Clone, Debug)]
struct Containment {
    lo: [f32; SURFACE_AXIS_COUNT],
    hi: [f32; SURFACE_AXIS_COUNT],
    unbiased: bool,
}

impl Containment {
    fn of(rect: ClimateRect) -> Self {
        Self {
            lo: rect.axes.map(|r| r.min.min(r.max)),
            hi: rect.axes.map(|r| r.min.max(r.max)),
            unbiased: rect.offset == 0.0,
        }
    }

    #[inline]
    fn contains(&self, v: &[f32; SURFACE_AXIS_COUNT]) -> bool {
        self.unbiased && (0..SURFACE_AXIS_COUNT).all(|a| !(v[a] < self.lo[a] || v[a] > self.hi[a]))
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
pub struct ClimateSampleCell {
    x: i32,
    y: i32,
    z: i32,
}

impl ClimateSampleCell {
    pub fn coords(self) -> (i32, i32, i32) {
        (self.x, self.y, self.z)
    }

    pub fn surface(wx: i32, wz: i32) -> Self {
        Self {
            x: wx.div_euclid(CLIMATE_SAMPLE_CELL_X),
            y: 0,
            z: wz.div_euclid(CLIMATE_SAMPLE_CELL_Z),
        }
    }

    pub const fn at_surface_indices(x: i32, z: i32) -> Self {
        Self { x, y: 0, z }
    }

    pub fn origin(self) -> (i32, i32, i32) {
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
pub struct ClimateSample {
    pub cell: ClimateSampleCell,
    pub climate: SurfaceClimate,
}

#[derive(Copy, Clone, Debug)]
pub struct ClimateSampler<'a> {
    graph: &'a ScalarGraph,
}

impl<'a> ClimateSampler<'a> {
    pub fn new(graph: &'a ScalarGraph) -> Self {
        Self { graph }
    }

    pub fn sample_surface_cell(self, cell: ClimateSampleCell) -> Option<ClimateSample> {
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
    use crate::graph::{Axis, Channel};

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
                biome: Biome::SNOWY_TUNDRA,
                rectangles: COLD,
            },
            BiomeClimateEntry {
                biome: Biome::DESERT,
                rectangles: WARM,
            },
        ]);

        assert_eq!(
            index.classify_surface(SurfaceClimate::new(0.50, 0.0, 0.0, 0.0, 0.0)),
            Some(Biome::DESERT)
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
    fn non_empty_index_always_returns_a_biome() {
        const RECTANGLES: &[ClimateRect] = &[test_rect(0.25, 0.75)];
        let index = BiomeClimateIndex::new(&[BiomeClimateEntry {
            biome: Biome::PLAINS,
            rectangles: RECTANGLES,
        }]);

        assert_eq!(
            index.classify_surface(SurfaceClimate::new(99.0, -50.0, 7.0, 4.0, 2.0)),
            Some(Biome::PLAINS)
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
                biome: Biome::PLAINS,
                rectangles: BROAD,
            },
            BiomeClimateEntry {
                biome: Biome::MEADOW,
                rectangles: SPECIFIC,
            },
        ]);
        let climate = SurfaceClimate::new(0.0, 0.0, 0.0, 0.0, 0.0);

        assert_eq!(index.classify_surface(climate), Some(Biome::MEADOW));
        assert_eq!(
            index.classify_surface_bruteforce(climate),
            Some(Biome::MEADOW)
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
