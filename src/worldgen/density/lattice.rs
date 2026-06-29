//! Coarse density lattice sampling for the staged surface worldgen rewrite.
//!
//! A lattice samples a named scalar graph channel at shared, world-anchored cell
//! corners, then provides trilinear per-voxel densities for a bounded region.
//! The live Stage 6A surface fill samples `master_density` through this lattice.
//!
//! Surface detection belongs after density evaluation. The stage-3
//! `surface_detection` graph channel remains only a placeholder; this module's
//! pure scan derives top solid surfaces from interpolated density sign
//! (`> 0.0` solid, `<= 0.0` air) without writing blocks.

use crate::chunk::{CHUNK_SX, CHUNK_SY, CHUNK_SZ};

use super::super::graph::{SamplePoint, ScalarGraph};

pub(crate) const DEFAULT_LATTICE_CELL_XZ: usize = 4;
pub(crate) const DEFAULT_LATTICE_CELL_Y: usize = 8;
const SURFACE_EMPTY_SEGMENT_EPSILON: f64 = 1.0e-12;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct DensityLatticeCellSize {
    x: usize,
    y: usize,
    z: usize,
}

impl DensityLatticeCellSize {
    pub(crate) const DEFAULT: Self = Self {
        x: DEFAULT_LATTICE_CELL_XZ,
        y: DEFAULT_LATTICE_CELL_Y,
        z: DEFAULT_LATTICE_CELL_XZ,
    };

    pub(crate) fn new(x: usize, y: usize, z: usize) -> Self {
        assert!(x > 0, "density lattice X cell size must be positive");
        assert!(y > 0, "density lattice Y cell size must be positive");
        assert!(z > 0, "density lattice Z cell size must be positive");
        Self { x, y, z }
    }

    pub(crate) const fn x(self) -> usize {
        self.x
    }

    pub(crate) const fn y(self) -> usize {
        self.y
    }

    pub(crate) const fn z(self) -> usize {
        self.z
    }
}

impl Default for DensityLatticeCellSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct DensityLatticeBounds {
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    size_x: usize,
    size_y: usize,
    size_z: usize,
}

impl DensityLatticeBounds {
    pub(crate) fn new(
        origin_x: i32,
        origin_y: i32,
        origin_z: i32,
        size_x: usize,
        size_y: usize,
        size_z: usize,
    ) -> Self {
        assert!(size_x > 0, "density lattice X bounds must be non-empty");
        assert!(size_y > 0, "density lattice Y bounds must be non-empty");
        assert!(size_z > 0, "density lattice Z bounds must be non-empty");
        assert!(
            origin_y >= 0,
            "density lattice voxel bounds must stay within world Y 0..255"
        );
        let end_y = axis_end(origin_y, size_y, "density lattice Y bounds");
        assert!(
            end_y <= CHUNK_SY as i32,
            "density lattice voxel bounds must stay within world Y 0..255"
        );

        Self {
            origin_x,
            origin_y,
            origin_z,
            size_x,
            size_y,
            size_z,
        }
    }

    pub(crate) fn chunk(cx: i32, cz: i32) -> Self {
        Self::new(
            cx * CHUNK_SX as i32,
            0,
            cz * CHUNK_SZ as i32,
            CHUNK_SX,
            CHUNK_SY,
            CHUNK_SZ,
        )
    }

    pub(crate) const fn origin(self) -> (i32, i32, i32) {
        (self.origin_x, self.origin_y, self.origin_z)
    }

    pub(crate) const fn size(self) -> (usize, usize, usize) {
        (self.size_x, self.size_y, self.size_z)
    }

    pub(crate) fn voxel_count(self) -> usize {
        self.size_x * self.size_y * self.size_z
    }

    pub(crate) fn column_count(self) -> usize {
        self.size_x * self.size_z
    }

    pub(crate) fn world_x(self, local_x: usize) -> i32 {
        assert!(
            local_x < self.size_x,
            "density lattice local X must be within bounds"
        );
        axis_coord(self.origin_x, local_x, "density lattice local X")
    }

    pub(crate) fn world_y(self, local_y: usize) -> i32 {
        assert!(
            local_y < self.size_y,
            "density lattice local Y must be within bounds"
        );
        axis_coord(self.origin_y, local_y, "density lattice local Y")
    }

    pub(crate) fn world_z(self, local_z: usize) -> i32 {
        assert!(
            local_z < self.size_z,
            "density lattice local Z must be within bounds"
        );
        axis_coord(self.origin_z, local_z, "density lattice local Z")
    }

    pub(crate) fn local_index(self, x: usize, y: usize, z: usize) -> usize {
        assert!(
            x < self.size_x && y < self.size_y && z < self.size_z,
            "density lattice local voxel index must be within bounds"
        );
        y * self.size_x * self.size_z + z * self.size_x + x
    }

    pub(crate) fn column_index(self, x: usize, z: usize) -> usize {
        assert!(
            x < self.size_x && z < self.size_z,
            "density lattice local column index must be within bounds"
        );
        z * self.size_x + x
    }

    fn contains_world(self, wx: i32, wy: i32, wz: i32) -> bool {
        axis_contains(self.origin_x, self.size_x, wx)
            && axis_contains(self.origin_y, self.size_y, wy)
            && axis_contains(self.origin_z, self.size_z, wz)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DensityLattice {
    bounds: DensityLatticeBounds,
    cell: DensityLatticeCellSize,
    sample_x: SampleAxis,
    sample_y: SampleAxis,
    sample_z: SampleAxis,
    samples: Vec<f64>,
}

impl DensityLattice {
    pub(crate) fn sample_channel(
        graph: &ScalarGraph,
        channel: impl AsRef<str>,
        bounds: DensityLatticeBounds,
        cell: DensityLatticeCellSize,
    ) -> Option<Self> {
        let channel = channel.as_ref();
        let root = graph.channel_node(channel)?;

        let sample_x = SampleAxis::cover(bounds.origin_x, bounds.size_x, cell.x);
        let sample_y = SampleAxis::cover(bounds.origin_y, bounds.size_y, cell.y);
        let sample_z = SampleAxis::cover(bounds.origin_z, bounds.size_z, cell.z);
        let mut samples = vec![0.0; sample_x.count * sample_y.count * sample_z.count];
        let mut cache = graph.evaluation_cache();

        for sz in 0..sample_z.count {
            let wz = sample_z.sample_coord(sz) as f64;
            for sx in 0..sample_x.count {
                let wx = sample_x.sample_coord(sx) as f64;
                cache.begin_y_invariant_column(graph);
                for sy in 0..sample_y.count {
                    let wy = sample_y.sample_coord(sy) as f64;
                    samples[(sy * sample_z.count + sz) * sample_x.count + sx] =
                        graph.evaluate_node_cached(root, SamplePoint::new(wx, wy, wz), &mut cache);
                }
            }
        }

        Some(Self {
            bounds,
            cell,
            sample_x,
            sample_y,
            sample_z,
            samples,
        })
    }

    pub(crate) fn sample_chunk(
        graph: &ScalarGraph,
        channel: impl AsRef<str>,
        cx: i32,
        cz: i32,
    ) -> Option<Self> {
        Self::sample_channel(
            graph,
            channel,
            DensityLatticeBounds::chunk(cx, cz),
            DensityLatticeCellSize::default(),
        )
    }

    pub(crate) const fn bounds(&self) -> DensityLatticeBounds {
        self.bounds
    }

    pub(crate) const fn cell_size(&self) -> DensityLatticeCellSize {
        self.cell
    }

    pub(crate) fn sample_counts(&self) -> (usize, usize, usize) {
        (
            self.sample_x.count,
            self.sample_y.count,
            self.sample_z.count,
        )
    }

    pub(crate) fn sample_world_origin(&self) -> (i32, i32, i32) {
        (
            self.sample_x.origin,
            self.sample_y.origin,
            self.sample_z.origin,
        )
    }

    /// Last sampled lattice corner. Full-height chunk lattices end at Y 256,
    /// while voxel density queries remain bounded to world Y 0..255.
    pub(crate) fn sample_world_max(&self) -> (i32, i32, i32) {
        (
            self.sample_x.last_coord(),
            self.sample_y.last_coord(),
            self.sample_z.last_coord(),
        )
    }

    pub(crate) fn density_at_local(&self, x: usize, y: usize, z: usize) -> f64 {
        let wx = self.bounds.world_x(x);
        let wy = self.bounds.world_y(y);
        let wz = self.bounds.world_z(z);
        self.interpolate_at_world(wx, wy, wz)
    }

    pub(crate) fn density_at_world(&self, wx: i32, wy: i32, wz: i32) -> f64 {
        assert!(
            self.bounds.contains_world(wx, wy, wz),
            "density lattice world lookup must be inside voxel bounds"
        );
        self.interpolate_at_world(wx, wy, wz)
    }

    pub(crate) fn solid_at_local(&self, x: usize, y: usize, z: usize) -> bool {
        density_is_solid(self.density_at_local(x, y, z))
    }

    pub(crate) fn solid_at_world(&self, wx: i32, wy: i32, wz: i32) -> bool {
        density_is_solid(self.density_at_world(wx, wy, wz))
    }

    pub(crate) fn for_each_solid(&self, mut visit: impl FnMut(usize, usize, usize, bool)) {
        for y in 0..self.bounds.size_y {
            for z in 0..self.bounds.size_z {
                for x in 0..self.bounds.size_x {
                    visit(x, y, z, self.solid_at_local(x, y, z));
                }
            }
        }
    }

    pub(crate) fn solid_mask(&self) -> Vec<bool> {
        let bounds = self.bounds;
        let mut mask = vec![false; bounds.voxel_count()];
        self.for_each_solid(|x, y, z, solid| {
            mask[bounds.local_index(x, y, z)] = solid;
        });
        mask
    }

    pub(crate) fn top_solid_surface(&self, x: usize, z: usize) -> Option<i32> {
        assert!(
            x < self.bounds.size_x && z < self.bounds.size_z,
            "density lattice surface column must be within bounds"
        );
        let (sx, tx) = self.sample_x.position(self.bounds.world_x(x));
        let (sz, tz) = self.sample_z.position(self.bounds.world_z(z));
        let y_segments = self.surface_y_segments_top_down();
        self.top_solid_surface_at_sample_position(sx, smooth_xz(tx), sz, smooth_xz(tz), &y_segments)
    }

    pub(crate) fn top_solid_surfaces(&self) -> Vec<Option<i32>> {
        let bounds = self.bounds;
        let mut surfaces = vec![None; bounds.column_count()];
        let x_positions =
            self.surface_axis_positions(self.sample_x, bounds.origin_x, bounds.size_x);
        let z_positions =
            self.surface_axis_positions(self.sample_z, bounds.origin_z, bounds.size_z);
        let y_segments = self.surface_y_segments_top_down();

        for z in 0..bounds.size_z {
            let (sz, tz) = z_positions[z];
            for x in 0..bounds.size_x {
                let (sx, tx) = x_positions[x];
                surfaces[bounds.column_index(x, z)] =
                    self.top_solid_surface_at_sample_position(sx, tx, sz, tz, &y_segments);
            }
        }
        surfaces
    }

    fn surface_axis_positions(
        &self,
        sample_axis: SampleAxis,
        origin: i32,
        size: usize,
    ) -> Vec<(usize, f64)> {
        (0..size)
            .map(|local| {
                let (sample, t) =
                    sample_axis.position(axis_coord(origin, local, "density lattice surface axis"));
                (sample, smooth_xz(t))
            })
            .collect()
    }

    fn surface_y_segments_top_down(&self) -> Vec<SurfaceYSegment> {
        let bounds_min = self.bounds.origin_y;
        let bounds_max = axis_end(
            self.bounds.origin_y,
            self.bounds.size_y,
            "density lattice surface Y bounds",
        ) - 1;

        (0..self.sample_y.count - 1)
            .rev()
            .filter_map(|sy| {
                let sample_y = self.sample_y.sample_coord(sy);
                let segment_min = sample_y.max(bounds_min);
                let segment_max = (sample_y + cell_i32(self.sample_y.cell) - 1).min(bounds_max);
                (segment_min <= segment_max).then_some(SurfaceYSegment {
                    sy,
                    sample_y,
                    min_y: segment_min,
                    max_y: segment_max,
                })
            })
            .collect()
    }

    fn top_solid_surface_at_sample_position(
        &self,
        sx: usize,
        tx: f64,
        sz: usize,
        tz: f64,
        y_segments: &[SurfaceYSegment],
    ) -> Option<i32> {
        let mut upper_plane = None;

        for segment in y_segments {
            let top = upper_plane
                .filter(|plane: &SurfaceColumnPlane| plane.sy == segment.sy + 1)
                .unwrap_or_else(|| self.surface_column_plane(sx, tx, segment.sy + 1, sz));
            let bottom = self.surface_column_plane(sx, tx, segment.sy, sz);

            let top_density =
                self.surface_segment_density(bottom, top, tz, segment.sample_y, segment.max_y);
            if density_is_solid(top_density) {
                return Some(segment.max_y);
            }
            if segment.min_y == segment.max_y {
                upper_plane = Some(bottom);
                continue;
            }

            let bottom_density =
                self.surface_segment_density(bottom, top, tz, segment.sample_y, segment.min_y);
            if top_density <= -SURFACE_EMPTY_SEGMENT_EPSILON
                && bottom_density <= -SURFACE_EMPTY_SEGMENT_EPSILON
            {
                upper_plane = Some(bottom);
                continue;
            }

            for wy in (segment.min_y..segment.max_y).rev() {
                if density_is_solid(self.surface_segment_density(
                    bottom,
                    top,
                    tz,
                    segment.sample_y,
                    wy,
                )) {
                    return Some(wy);
                }
            }
            upper_plane = Some(bottom);
        }
        None
    }

    fn surface_column_plane(&self, sx: usize, tx: f64, sy: usize, sz: usize) -> SurfaceColumnPlane {
        SurfaceColumnPlane {
            sy,
            x0: lerp(self.sample(sx, sy, sz), self.sample(sx + 1, sy, sz), tx),
            x1: lerp(
                self.sample(sx, sy, sz + 1),
                self.sample(sx + 1, sy, sz + 1),
                tx,
            ),
        }
    }

    fn surface_segment_density(
        &self,
        bottom: SurfaceColumnPlane,
        top: SurfaceColumnPlane,
        tz: f64,
        sample_y: i32,
        wy: i32,
    ) -> f64 {
        let ty = (wy - sample_y) as f64 / self.cell.y as f64;
        let y0 = lerp(bottom.x0, top.x0, ty);
        let y1 = lerp(bottom.x1, top.x1, ty);
        lerp(y0, y1, tz)
    }

    fn interpolate_at_world(&self, wx: i32, wy: i32, wz: i32) -> f64 {
        let (sx, tx) = self.sample_x.position(wx);
        let (sy, ty) = self.sample_y.position(wy);
        let (sz, tz) = self.sample_z.position(wz);
        let (tx, tz) = (smooth_xz(tx), smooth_xz(tz));

        let c000 = self.sample(sx, sy, sz);
        let c100 = self.sample(sx + 1, sy, sz);
        let c010 = self.sample(sx, sy + 1, sz);
        let c110 = self.sample(sx + 1, sy + 1, sz);
        let c001 = self.sample(sx, sy, sz + 1);
        let c101 = self.sample(sx + 1, sy, sz + 1);
        let c011 = self.sample(sx, sy + 1, sz + 1);
        let c111 = self.sample(sx + 1, sy + 1, sz + 1);

        let x00 = lerp(c000, c100, tx);
        let x10 = lerp(c010, c110, tx);
        let x01 = lerp(c001, c101, tx);
        let x11 = lerp(c011, c111, tx);
        let y0 = lerp(x00, x10, ty);
        let y1 = lerp(x01, x11, ty);
        lerp(y0, y1, tz)
    }

    fn sample(&self, sx: usize, sy: usize, sz: usize) -> f64 {
        debug_assert!(sx < self.sample_x.count);
        debug_assert!(sy < self.sample_y.count);
        debug_assert!(sz < self.sample_z.count);
        self.samples[(sy * self.sample_z.count + sz) * self.sample_x.count + sx]
    }
}

pub(crate) fn density_is_solid(density: f64) -> bool {
    density > 0.0
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct SampleAxis {
    origin: i32,
    count: usize,
    cell: usize,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct SurfaceYSegment {
    sy: usize,
    sample_y: i32,
    min_y: i32,
    max_y: i32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct SurfaceColumnPlane {
    sy: usize,
    x0: f64,
    x1: f64,
}

impl SampleAxis {
    fn cover(origin: i32, size: usize, cell: usize) -> Self {
        assert!(cell > 0, "density lattice cell size must be positive");
        let last_voxel = axis_coord(origin, size - 1, "density lattice sample bounds");
        let sample_origin = floor_to_cell(origin, cell);
        let sample_end = floor_to_cell(last_voxel, cell)
            .checked_add(cell_i32(cell))
            .expect("density lattice sample bounds must fit i32 world coordinates");
        let count = ((sample_end - sample_origin) / cell_i32(cell) + 1) as usize;

        Self {
            origin: sample_origin,
            count,
            cell,
        }
    }

    fn sample_coord(self, index: usize) -> i32 {
        assert!(
            index < self.count,
            "density lattice sample index must be within bounds"
        );
        stepped_axis_coord(
            self.origin,
            index,
            self.cell,
            "density lattice sample coordinate",
        )
    }

    fn last_coord(self) -> i32 {
        self.sample_coord(self.count - 1)
    }

    fn position(self, coord: i32) -> (usize, f64) {
        let lower = floor_to_cell(coord, self.cell);
        let offset = lower - self.origin;
        debug_assert!(offset >= 0);
        let index = (offset / cell_i32(self.cell)) as usize;
        debug_assert!(index + 1 < self.count);
        let t = (coord - lower) as f64 / self.cell as f64;
        (index, t)
    }
}

fn axis_contains(origin: i32, size: usize, coord: i32) -> bool {
    let coord = i64::from(coord);
    let start = i64::from(origin);
    let end = start + size as i64;
    coord >= start && coord < end
}

fn axis_end(origin: i32, size: usize, context: &str) -> i32 {
    origin
        .checked_add(i32::try_from(size).expect(context))
        .expect(context)
}

fn axis_coord(origin: i32, local: usize, context: &str) -> i32 {
    origin
        .checked_add(i32::try_from(local).expect(context))
        .expect(context)
}

fn stepped_axis_coord(origin: i32, index: usize, step: usize, context: &str) -> i32 {
    let offset = index
        .checked_mul(step)
        .and_then(|value| i32::try_from(value).ok())
        .expect(context);
    origin.checked_add(offset).expect(context)
}

fn floor_to_cell(coord: i32, cell: usize) -> i32 {
    coord.div_euclid(cell_i32(cell)) * cell_i32(cell)
}

fn cell_i32(cell: usize) -> i32 {
    i32::try_from(cell).expect("density lattice cell size must fit i32")
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Smoothstep the horizontal (X/Z) interpolation parameter so reconstructed
/// densities are C1-continuous across cell corners. Plain bilinear is only C0:
/// its gradient jumps at every cell boundary, which a smooth (detail-noise-free)
/// macro surface exposes as a regular grid of flat facets in hillshade. Y stays
/// linear on purpose — `master_density` is exactly linear in Y, so a linear Y
/// blend reconstructs the depth crossing without error.
fn smooth_xz(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{CHUNK_SX, CHUNK_SY, CHUNK_SZ};
    use crate::worldgen::graph::{Axis, Channel, SampledScalarField};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const CHANNEL: &str = "master_density";

    #[derive(Copy, Clone, Debug)]
    struct LinearField {
        x: f64,
        y: f64,
        z: f64,
        offset: f64,
    }

    impl LinearField {
        fn value(&self, point: SamplePoint) -> f64 {
            self.x * point.x + self.y * point.y + self.z * point.z + self.offset
        }
    }

    impl SampledScalarField for LinearField {
        fn sample(&self, point: SamplePoint) -> f64 {
            self.value(point)
        }
    }

    #[derive(Debug)]
    struct CurvedField;

    impl SampledScalarField for CurvedField {
        fn sample(&self, point: SamplePoint) -> f64 {
            point.x * point.x * 0.013 + point.y * 0.37 - point.z * point.z * 0.019 + 3.0
        }
    }

    #[derive(Debug)]
    struct PlaneField {
        surface_y: f64,
    }

    impl SampledScalarField for PlaneField {
        fn sample(&self, point: SamplePoint) -> f64 {
            self.surface_y - point.y
        }
    }

    #[derive(Debug)]
    struct ConstantField(f64);

    impl SampledScalarField for ConstantField {
        fn sample(&self, _point: SamplePoint) -> f64 {
            self.0
        }
    }

    #[derive(Debug)]
    struct HorizontalCountingField {
        samples: Arc<AtomicUsize>,
    }

    impl SampledScalarField for HorizontalCountingField {
        fn sample(&self, point: SamplePoint) -> f64 {
            self.samples.fetch_add(1, Ordering::Relaxed);
            point.x * 0.25 - point.z * 0.125
        }

        fn depends_on_y(&self) -> bool {
            false
        }
    }

    fn graph_with(field: impl SampledScalarField + 'static) -> ScalarGraph {
        let mut graph = ScalarGraph::new();
        let node = graph.sampled_field(field);
        graph.set_channel(Channel::new(CHANNEL), node);
        graph
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn interpolation_is_corner_exact_y_linear_and_xz_smoothstep() {
        let field = LinearField {
            x: 1.25,
            y: -0.5,
            z: 2.0,
            offset: 7.0,
        };
        let graph = graph_with(LinearField { ..field });
        let bounds = DensityLatticeBounds::new(-3, 0, 5, 20, CHUNK_SY, 18);
        let lattice = DensityLattice::sample_channel(
            &graph,
            CHANNEL,
            bounds,
            DensityLatticeCellSize::new(4, 8, 4),
        )
        .unwrap();
        let point = |x, y, z| {
            SamplePoint::new(
                bounds.world_x(x) as f64,
                bounds.world_y(y) as f64,
                bounds.world_z(z) as f64,
            )
        };

        // Cell corners (world XZ multiples of 4, world Y multiples of 8)
        // reconstruct the channel exactly: smoothstep fixes the endpoints.
        for (x, y, z) in [(3, 0, 3), (7, 8, 7), (11, 16, 3)] {
            assert_close(lattice.density_at_local(x, y, z), field.value(point(x, y, z)));
        }

        // Y interpolation stays linear: at an XZ corner any Y is exact.
        assert_close(lattice.density_at_local(3, 4, 3), field.value(point(3, 4, 3)));

        // XZ interpolation is smoothstep, not linear: at a quarter into the cell
        // the X blend is smooth(0.25)=0.15625, landing short of the linear point.
        let x_lo = field.value(point(3, 0, 3));
        let x_hi = field.value(point(7, 0, 3));
        let expected = x_lo + (x_hi - x_lo) * smooth_xz(0.25);
        assert_close(lattice.density_at_local(4, 0, 3), expected);
    }

    #[test]
    fn overlapping_regions_sample_identical_world_voxels() {
        let graph = graph_with(CurvedField);
        let chunk_bounds = DensityLatticeBounds::chunk(1, -2);
        let chunk_lattice =
            DensityLattice::sample_channel(&graph, CHANNEL, chunk_bounds, Default::default())
                .unwrap();
        let larger_bounds = DensityLatticeBounds::new(9, 0, -41, 48, CHUNK_SY, 48);
        let larger_lattice =
            DensityLattice::sample_channel(&graph, CHANNEL, larger_bounds, Default::default())
                .unwrap();

        for y in [0, 1, 7, 8, 63, 127, 255] {
            for z in [0, 3, 8, 15] {
                for x in [0, 1, 4, 15] {
                    let wx = chunk_bounds.world_x(x);
                    let wy = chunk_bounds.world_y(y);
                    let wz = chunk_bounds.world_z(z);

                    assert_close(
                        chunk_lattice.density_at_local(x, y, z),
                        larger_lattice.density_at_world(wx, wy, wz),
                    );
                }
            }
        }
    }

    #[test]
    fn non_aligned_negative_regions_sample_identical_world_voxels() {
        let graph = graph_with(CurvedField);
        let small_bounds = DensityLatticeBounds::new(-23, 0, -19, 13, CHUNK_SY, 11);
        let large_bounds = DensityLatticeBounds::new(-31, 0, -27, 37, CHUNK_SY, 35);
        let small =
            DensityLattice::sample_channel(&graph, CHANNEL, small_bounds, Default::default())
                .unwrap();
        let large =
            DensityLattice::sample_channel(&graph, CHANNEL, large_bounds, Default::default())
                .unwrap();

        for y in [0, 6, 8, 71, 255] {
            for z in [0, 1, 5, 10] {
                for x in [0, 2, 7, 12] {
                    let wx = small_bounds.world_x(x);
                    let wy = small_bounds.world_y(y);
                    let wz = small_bounds.world_z(z);

                    assert_close(
                        small.density_at_local(x, y, z),
                        large.density_at_world(wx, wy, wz),
                    );
                }
            }
        }
    }

    #[test]
    fn sampling_reuses_y_invariant_nodes_per_lattice_column() {
        let samples = Arc::new(AtomicUsize::new(0));
        let mut graph = ScalarGraph::new();
        let horizontal = graph.sampled_field(HorizontalCountingField {
            samples: samples.clone(),
        });
        let y = graph.axis(Axis::Y);
        let y_scale = graph.constant(0.5);
        let y_scaled = graph.multiply(y, y_scale);
        let output = graph.add(horizontal, y_scaled);
        graph.set_channel(Channel::new(CHANNEL), output);

        let bounds = DensityLatticeBounds::new(-2, 0, 3, 9, 17, 10);
        let lattice = DensityLattice::sample_channel(
            &graph,
            CHANNEL,
            bounds,
            DensityLatticeCellSize::new(4, 8, 4),
        )
        .unwrap();
        let (sample_x, _sample_y, sample_z) = lattice.sample_counts();

        assert_eq!(
            samples.load(Ordering::Relaxed),
            sample_x * sample_z,
            "horizontal subgraph should be sampled once per X/Z lattice column"
        );

        // Sample at columns where smoothstep coincides with the linear blend
        // (XZ cell corners / midpoints) so the closed-form expectation holds;
        // this test guards the per-column reuse and Y-linearity, not the XZ curve.
        for (x, y, z) in [(0, 0, 1), (2, 8, 5), (8, 16, 9)] {
            let wx = f64::from(bounds.world_x(x));
            let wy = f64::from(bounds.world_y(y));
            let wz = f64::from(bounds.world_z(z));
            let expected = wx * 0.25 - wz * 0.125 + wy * 0.5;

            assert_close(lattice.density_at_local(x, y, z), expected);
        }
    }

    #[test]
    fn solid_mask_and_top_surface_use_density_sign() {
        let graph = graph_with(PlaneField { surface_y: 2.0 });
        let lattice = DensityLattice::sample_chunk(&graph, CHANNEL, 0, 0).unwrap();

        assert!(lattice.solid_at_local(0, 0, 0));
        assert!(lattice.solid_at_local(15, 1, 15));
        assert!(!lattice.solid_at_local(0, 2, 0));
        assert!(!lattice.solid_at_local(0, 255, 0));

        let mask = lattice.solid_mask();
        assert_eq!(mask.len(), CHUNK_SX * CHUNK_SY * CHUNK_SZ);
        assert_eq!(
            mask.iter().filter(|&&solid| solid).count(),
            CHUNK_SX * CHUNK_SZ * 2
        );

        let surfaces = lattice.top_solid_surfaces();
        assert_eq!(surfaces.len(), CHUNK_SX * CHUNK_SZ);
        assert!(surfaces.iter().all(|surface| *surface == Some(1)));
        assert_eq!(lattice.top_solid_surface(3, 4), Some(1));
    }

    #[test]
    fn top_surfaces_match_full_density_scan_on_non_aligned_bounds() {
        let graph = graph_with(LinearField {
            x: 0.45,
            y: -0.9,
            z: -0.35,
            offset: 92.0,
        });
        let bounds = DensityLatticeBounds::new(-7, 11, 9, 21, 113, 17);
        let lattice = DensityLattice::sample_channel(
            &graph,
            CHANNEL,
            bounds,
            DensityLatticeCellSize::new(5, 7, 6),
        )
        .unwrap();

        let surfaces = lattice.top_solid_surfaces();
        assert_eq!(surfaces.len(), bounds.column_count());

        for z in 0..bounds.size_z {
            for x in 0..bounds.size_x {
                let expected = (0..bounds.size_y)
                    .rev()
                    .find(|&y| lattice.solid_at_local(x, y, z))
                    .map(|y| bounds.world_y(y));
                assert_eq!(surfaces[bounds.column_index(x, z)], expected);
                assert_eq!(lattice.top_solid_surface(x, z), expected);
            }
        }
    }

    #[test]
    fn default_cells_align_with_chunk_shape_and_shared_corners() {
        let graph = graph_with(ConstantField(1.0));
        let lattice = DensityLattice::sample_chunk(&graph, CHANNEL, 0, 0).unwrap();
        let cell = DensityLatticeCellSize::default();

        assert_eq!(cell, DensityLatticeCellSize::new(4, 8, 4));
        assert_eq!(CHUNK_SX % cell.x(), 0);
        assert_eq!(CHUNK_SY % cell.y(), 0);
        assert_eq!(CHUNK_SZ % cell.z(), 0);
        assert_eq!(lattice.bounds(), DensityLatticeBounds::chunk(0, 0));
        assert_eq!(lattice.cell_size(), cell);
        assert_eq!(
            lattice.sample_counts(),
            (
                CHUNK_SX / cell.x() + 1,
                CHUNK_SY / cell.y() + 1,
                CHUNK_SZ / cell.z() + 1
            )
        );
        assert_eq!(lattice.sample_world_origin(), (0, 0, 0));
        assert_eq!(
            lattice.sample_world_max(),
            (CHUNK_SX as i32, CHUNK_SY as i32, CHUNK_SZ as i32)
        );

        let negative = DensityLattice::sample_chunk(&graph, CHANNEL, -1, -1).unwrap();
        assert_eq!(negative.sample_world_origin(), (-16, 0, -16));
        assert_eq!(negative.sample_world_max(), (0, CHUNK_SY as i32, 0));
    }
}
