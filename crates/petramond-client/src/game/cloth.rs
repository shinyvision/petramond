//! Client-side cloth: every block row that names a `cloth.json` row hangs a sheet of
//! fabric, simulated here for presentation only. Points integrate with Verlet, hold
//! together through grid distance constraints, catch the wind on each triangle, and
//! collide with the replica's blocks and the presented bodies around them.
//!
//! Each fixed step runs as several substeps of one constraint pass each, which keeps
//! the fabric stiff and quiet for the same work as many passes over one big step.
//!
//! Contact treats the sheet as offset by `CONTACT_RADIUS` and keeps it that far from
//! every collision box through all the pairings two convex shapes meet at (cloth vertex
//! vs box, box corner vs cloth triangle, box edge vs cloth edge). Every vertex moves at
//! most `BOUND_RELAX` of its triangles' distance to the nearest box per substep, so no
//! point of a triangle can reach a box it was clear of: the sheet never passes through
//! blocks between its vertices, with no continuous collision detection.
//!
//! Nothing here is authoritative or deterministic; two clients may flutter a flag
//! differently. The wind is whatever the client mods set (`ClientClothWindSet`), or
//! [`DEFAULT_WIND`] when none does.

use petramond::world::{PlacedCloth, ReplicaWorld};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Aabb;
use petramond_world::cloth::{ClothDef, MAX_EXTENT};
use petramond_world::verlet::{self, closest};
use rustc_hash::FxHashMap;

/// East to west, in blocks per second.
pub(super) const DEFAULT_WIND: [f32; 2] = [-1.5, 0.0];

const STEP: f32 = 1.0 / 60.0;
const SUBSTEPS: usize = 6;
const MAX_STEPS_PER_FRAME: u32 = 3;
const GRAVITY: f32 = 9.8;
/// Force per unit area per block/s of wind pressing on a triangle face-on.
const AERO_NORMAL: f32 = 20.0;
/// The weaker drag along the fabric that keeps a sheet streaming downwind.
const AERO_TANGENT: f32 = 8.0;
/// Shear and bend links are much weaker than stretch: fabric keeps its length but
/// folds and skews freely, which is what lets a calm flag droop against its pole.
const SHEAR: f32 = 0.02;
const BEND: f32 = 0.005;
/// The sheet's offset from what it touches; also keeps it off block faces it rests on.
const CONTACT_RADIUS: f32 = 0.03;
/// How far a vertex looks for boxes when bounding its move; far from everything it may
/// move this much times [`BOUND_RELAX`] per substep.
const QUERY_RADIUS: f32 = 0.25;
/// Fraction of the clear distance a vertex may cover per substep; below one half so two
/// sides closing on each other can never meet.
const BOUND_RELAX: f32 = 0.45;
/// Clearance kept back from every bound, so a sheet creeping toward a box stops short
/// of where float rounding could close the last gap.
const BOUND_SLACK: f32 = 1e-4;
/// A body pushes fabric a little before its hull touches it.
const BODY_MARGIN: f32 = 0.06;
/// Collapsed width, as a fraction, a cloth starts from when unfurling clear of a block.
const FURLED: f32 = 0.04;
const MAX_GRID: usize = petramond_world::cloth::MAX_SEGMENTS as usize + 1;
/// A simulated flag keeps simulating this much past [`SIM_RADIUS`] before it hands
/// back to the far pose, so standing at the edge never flickers between the two.
const SIM_HYSTERESIS: i32 = 8;
/// The far pose's droop in still air and in a gale, and the wind speed over which it
/// lifts from one to the other.
const FAR_DROOP_MAX: f32 = 1.15;
const FAR_DROOP_MIN: f32 = 0.12;
const FAR_DROOP_FALLOFF: f32 = 0.75;
const FAR_RIPPLE: f32 = 0.06;
const FAR_WAVE_NUMBER: f32 = std::f32::consts::TAU / 1.2;
/// Beyond these distances a far flag drops to half, then a quarter, of its grid.
const FAR_HALF_GRID: f32 = 96.0;
const FAR_QUARTER_GRID: f32 = 160.0;
const SIM_RADIUS: i32 = 48;
const MAX_ACTIVE: usize = 64;

/// A presented body as an upright capsule, in world space.
#[derive(Copy, Clone, Debug)]
pub(super) struct BodyHull {
    pub feet: WorldPos,
    pub height: f32,
    pub radius: f32,
}

/// An axis-aligned collision box relative to the cloth's cell.
#[derive(Copy, Clone, Debug)]
struct Obstacle {
    min: Vec3,
    max: Vec3,
    corners: [Vec3; 8],
    /// Each of the 12 edges' bounds, for culling cloth edges far from them.
    edge_bounds: [(Vec3, Vec3); 12],
}

pub(super) struct ClothSim {
    pub(super) cell: IVec3,
    pub(super) def: &'static ClothDef,
    /// Positions relative to the owning cell's minimum corner.
    pos: Vec<Vec3>,
    old: Vec<Vec3>,
    prev: Vec<Vec3>,
    accel: Vec<Vec3>,
    clear: Vec<Vec3>,
    bound: Vec<f32>,
    obstacles: Vec<Obstacle>,
    full: Vec<bool>,
    tris: Box<[[usize; 3]]>,
    edges: Box<[[usize; 2]]>,
    phase: f32,
    #[cfg(test)]
    furls: u32,
}

impl ClothSim {
    /// A sim starting from the far pose at `time`, so a flag coming into simulation
    /// range carries on from the shape it was just drawn in.
    pub(super) fn new(cell: IVec3, def: &'static ClothDef, wind: [f32; 2], time: f32) -> Self {
        let [cols, rows] = def.segments.map(|s| usize::from(s) + 1);
        let mut sim = Self {
            cell,
            def,
            pos: Vec::new(),
            old: Vec::new(),
            prev: Vec::new(),
            accel: Vec::new(),
            clear: Vec::new(),
            bound: Vec::new(),
            obstacles: Vec::new(),
            full: Vec::new(),
            tris: triangles(cols, rows).collect(),
            edges: edges(cols, rows).collect(),
            phase: phase_of(cell),
            #[cfg(test)]
            furls: 0,
        };
        far_pose(def, cell, wind, time, [cols - 1, rows - 1], &mut sim.pos);
        sim.old.clone_from(&sim.pos);
        sim.prev.clone_from(&sim.pos);
        sim
    }

    /// Lays the sheet out downwind at `spread` of its width, at rest.
    fn lay_out(&mut self, wind: [f32; 2], spread: f32) {
        let def = self.def;
        let downwind = Vec3::new(wind[0], 0.0, wind[1]).normalize_or(Vec3::NEG_X);
        let [cols, rows] = def.segments.map(usize::from);
        let [du, dv] = [def.size[0] / cols as f32, def.size[1] / rows as f32];
        let anchor = Vec3::from_array(def.anchor);
        self.pos.clear();
        self.pos.extend((0..=rows).flat_map(|v| {
            (0..=cols).map(move |u| {
                anchor + downwind * (u as f32 * du * spread) - Vec3::Y * (v as f32 * dv)
            })
        }));
        self.old.clone_from(&self.pos);
        self.prev.clone_from(&self.pos);
    }

    #[inline]
    fn cols(&self) -> usize {
        usize::from(self.def.segments[0]) + 1
    }

    #[inline]
    fn rows(&self) -> usize {
        usize::from(self.def.segments[1]) + 1
    }

    #[inline]
    fn pinned(&self, i: usize) -> bool {
        i.is_multiple_of(self.cols())
    }

    fn pin(&self, i: usize) -> Vec3 {
        let dv = self.def.size[1] / f32::from(self.def.segments[1]);
        Vec3::from_array(self.def.anchor) - Vec3::Y * ((i / self.cols()) as f32 * dv)
    }

    /// Advances one fixed step. `boxes(cell)` answers collision boxes for a cell given
    /// relative to the owning cell; `bodies` are hulls already made relative too.
    pub(super) fn step(
        &mut self,
        time: f32,
        wind: [f32; 2],
        boxes: &impl Fn(IVec3) -> &'static [Aabb],
        bodies: &[(Vec3, f32, f32)],
    ) {
        let def = self.def;
        let h = STEP / SUBSTEPS as f32;
        self.prev.copy_from_slice(&self.pos);
        self.gather_obstacles(boxes);
        let mut accel = std::mem::take(&mut self.accel);
        self.wind_accel(time, wind, h, &mut accel);
        let gravity = Vec3::Y * (GRAVITY * def.gravity);
        let damping = def.damping.powf(1.0 / SUBSTEPS as f32);
        for _ in 0..SUBSTEPS {
            if !self.bound_moves() {
                // A block now cuts the sheet (placed through it, or the cloth spawned
                // inside one): furl back to the post and unfurl clear of it.
                self.lay_out(wind, FURLED);
                #[cfg(test)]
                {
                    self.furls += 1;
                }
                break;
            }
            self.clear.clone_from(&self.pos);
            let cols = self.cols();
            let points = self.pos.iter_mut().zip(&mut self.old).zip(&accel);
            for (i, ((x, x_old), a)) in points.enumerate() {
                if !i.is_multiple_of(cols) {
                    verlet::integrate(x, x_old, *a - gravity, h * h, damping);
                }
            }
            self.solve_links();
            self.contact_boxes();
            self.contact_bodies(bodies);
            for i in 0..self.pos.len() {
                if self.pinned(i) {
                    self.pos[i] = self.pin(i);
                    self.old[i] = self.pos[i];
                    continue;
                }
                let moved = self.pos[i] - self.clear[i];
                let len = moved.length();
                if len > self.bound[i] {
                    self.pos[i] = self.clear[i] + moved * (self.bound[i] / len);
                }
            }
        }
        self.accel = accel;
        if self
            .pos
            .iter()
            .any(|p| !p.is_finite() || p.length() > MAX_EXTENT * 2.0 + 2.0)
        {
            self.lay_out(wind, FURLED);
        }
    }

    /// Collects every box near the sheet. The post's own column is skipped across the
    /// pinned edge's span: the edge lives inside it. Full cubes merge greedily into
    /// larger boxes, so a wall or the ground is one box rather than a grid of seams
    /// whose corners and edges would each need testing.
    fn gather_obstacles(&mut self, boxes: &impl Fn(IVec3) -> &'static [Aabb]) {
        self.obstacles.clear();
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for p in &self.pos {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let reach = QUERY_RADIUS + BOUND_RELAX * QUERY_RADIUS * SUBSTEPS as f32;
        let (lo, hi) = (
            (lo - reach).floor().as_ivec3(),
            (hi + reach).floor().as_ivec3(),
        );
        let size = (hi - lo + IVec3::ONE).max(IVec3::ZERO);
        let anchor_y = self.def.anchor[1];
        let span = (anchor_y - self.def.size[1]).floor() as i32..=anchor_y.floor() as i32;
        let index = |c: IVec3| ((c.y * size.z + c.z) * size.x + c.x) as usize;
        self.full.clear();
        self.full.resize((size.x * size.y * size.z) as usize, false);
        for y in 0..size.y {
            for z in 0..size.z {
                for x in 0..size.x {
                    let cell = lo + IVec3::new(x, y, z);
                    if cell.x == 0 && cell.z == 0 && span.contains(&cell.y) {
                        continue;
                    }
                    let cell_boxes = boxes(cell);
                    if is_full_cube(cell_boxes) {
                        self.full[index(IVec3::new(x, y, z))] = true;
                        continue;
                    }
                    let base = cell.as_vec3();
                    for b in cell_boxes {
                        self.push_obstacle(
                            base + Vec3::from_array(b.min),
                            base + Vec3::from_array(b.max),
                        );
                    }
                }
            }
        }
        for y in 0..size.y {
            for z in 0..size.z {
                for x in 0..size.x {
                    if !self.full[index(IVec3::new(x, y, z))] {
                        continue;
                    }
                    let run = |full: &[bool], from: IVec3, to: IVec3| {
                        (from.y..=to.y).all(|y| {
                            (from.z..=to.z)
                                .all(|z| (from.x..=to.x).all(|x| full[index(IVec3::new(x, y, z))]))
                        })
                    };
                    let start = IVec3::new(x, y, z);
                    let mut end = start;
                    while end.x + 1 < size.x
                        && run(
                            &self.full,
                            IVec3::new(end.x + 1, y, z),
                            IVec3::new(end.x + 1, y, z),
                        )
                    {
                        end.x += 1;
                    }
                    while end.z + 1 < size.z
                        && run(
                            &self.full,
                            IVec3::new(x, y, end.z + 1),
                            IVec3::new(end.x, y, end.z + 1),
                        )
                    {
                        end.z += 1;
                    }
                    while end.y + 1 < size.y
                        && run(
                            &self.full,
                            IVec3::new(x, end.y + 1, z),
                            IVec3::new(end.x, end.y + 1, end.z),
                        )
                    {
                        end.y += 1;
                    }
                    for yy in start.y..=end.y {
                        for zz in start.z..=end.z {
                            for xx in start.x..=end.x {
                                self.full[index(IVec3::new(xx, yy, zz))] = false;
                            }
                        }
                    }
                    self.push_obstacle((lo + start).as_vec3(), (lo + end + IVec3::ONE).as_vec3());
                }
            }
        }
    }

    fn push_obstacle(&mut self, min: Vec3, max: Vec3) {
        let corners = closest::box_corners(min, max);
        self.obstacles.push(Obstacle {
            min,
            max,
            corners,
            edge_bounds: closest::BOX_EDGES
                .map(|(i, j)| (corners[i].min(corners[j]), corners[i].max(corners[j]))),
        });
    }

    /// Each vertex's allowed move this substep: [`BOUND_RELAX`] of the smallest
    /// distance from any triangle it belongs to to any box. False when a triangle
    /// already overlaps a box.
    fn bound_moves(&mut self) -> bool {
        self.bound.clear();
        self.bound
            .resize(self.pos.len(), QUERY_RADIUS * BOUND_RELAX);
        let Some((all_lo, all_hi)) = self
            .obstacles
            .iter()
            .map(|o| (o.min, o.max))
            .reduce(|(lo, hi), (min, max)| (lo.min(min), hi.max(max)))
        else {
            return true;
        };
        for v in &mut self.bound {
            *v = QUERY_RADIUS;
        }
        for t in 0..self.tris.len() {
            let [a, b, c] = self.tris[t];
            let (pa, pb, pc) = (self.pos[a], self.pos[b], self.pos[c]);
            let (lo, hi) = (pa.min(pb).min(pc), pa.max(pb).max(pc));
            if closest::box_box_distance(lo, hi, all_lo, all_hi) >= QUERY_RADIUS {
                continue;
            }
            let mut d = QUERY_RADIUS;
            for o in &self.obstacles {
                // The bounds' gap is a lower bound on the true distance, so it bounds
                // safely on its own; the exact query is only worth it when the gap is
                // too small to move by, as at a corner the triangle slants across.
                let gap = closest::box_box_distance(lo, hi, o.min, o.max);
                if gap >= d {
                    continue;
                }
                d = if gap >= CONTACT_RADIUS * 0.5 {
                    gap
                } else {
                    d.min(closest::triangle_box_distance(pa, pb, pc, o.min, o.max))
                };
            }
            if d <= 0.0 && !(self.pinned(a) && self.pinned(b) && self.pinned(c)) {
                return false;
            }
            let clear = (d - BOUND_SLACK).max(0.0);
            for i in [a, b, c] {
                self.bound[i] = self.bound[i].min(clear);
            }
        }
        for b in &mut self.bound {
            *b *= BOUND_RELAX;
        }
        true
    }

    fn solve_links(&mut self) {
        let def = self.def;
        let (cols, rows) = (self.cols(), self.rows());
        let [du, dv] = [
            def.size[0] / f32::from(def.segments[0]),
            def.size[1] / f32::from(def.segments[1]),
        ];
        let diag = du.hypot(dv);
        let k = def.stiffness;
        let pos = &mut self.pos;
        for v in 0..rows {
            for u in 0..cols {
                let i = v * cols + u;
                if u + 1 < cols {
                    link(pos, i, i + 1, du, [u == 0, false], k);
                }
                if u + 2 < cols {
                    link(pos, i, i + 2, du * 2.0, [u == 0, false], k * BEND);
                }
                if v + 1 < rows {
                    link(pos, i, i + cols, dv, [u == 0; 2], k);
                    if u + 1 < cols {
                        link(pos, i, i + cols + 1, diag, [u == 0, false], k * SHEAR);
                    }
                    if u > 0 {
                        link(pos, i, i + cols - 1, diag, [false, u == 1], k * SHEAR);
                    }
                }
                if v + 2 < rows {
                    link(pos, i, i + 2 * cols, dv * 2.0, [u == 0; 2], k * BEND);
                }
            }
        }
    }

    /// Projects the sheet out to [`CONTACT_RADIUS`] from every box through the three
    /// pairings: each correction runs along the separating direction of the closest
    /// features, orthogonal to the face it acts on.
    fn contact_boxes(&mut self) {
        let r = CONTACT_RADIUS;
        let weight = |sim: &Self, i: usize| if sim.pinned(i) { 0.0 } else { 1.0 };
        for oi in 0..self.obstacles.len() {
            let o = self.obstacles[oi];
            for i in 0..self.pos.len() {
                let p = self.pos[i];
                if self.pinned(i) {
                    continue;
                }
                let c = closest::closest_on_box(p, o.min, o.max);
                let d = (p - c).length();
                if d < r && d > 1e-6 {
                    self.pos[i] = c + (p - c) * (r / d);
                }
            }
            for t in 0..self.tris.len() {
                let [a, b, c] = self.tris[t];
                let (pa, pb, pc) = (self.pos[a], self.pos[b], self.pos[c]);
                let (lo, hi) = (pa.min(pb).min(pc), pa.max(pb).max(pc));
                if !within(lo, hi, o.min, o.max, r) {
                    continue;
                }
                if planes_near(lo, hi, &o, r) < 3 {
                    continue;
                }
                let w = [weight(self, a), weight(self, b), weight(self, c)];
                for q in o.corners {
                    if !within(lo, hi, q, q, r) {
                        continue;
                    }
                    let (pt, bary) = closest::closest_on_triangle(q, pa, pb, pc);
                    let d = (pt - q).length();
                    if d >= r || bary.iter().any(|&x| x <= 1e-4) {
                        continue;
                    }
                    let n = if d > 1e-6 {
                        (pt - q) / d
                    } else {
                        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
                        let was = (self.clear[a] + self.clear[b] + self.clear[c]) / 3.0;
                        if n.dot(was - q) < 0.0 {
                            -n
                        } else {
                            n
                        }
                    };
                    let denom: f32 = (0..3).map(|k| w[k] * bary[k] * bary[k]).sum();
                    if denom <= 1e-8 {
                        continue;
                    }
                    let s = (r - d) / denom;
                    for (k, i) in [a, b, c].into_iter().enumerate() {
                        self.pos[i] += n * (s * w[k] * bary[k]);
                    }
                }
            }
            for e in 0..self.edges.len() {
                let [i, j] = self.edges[e];
                let (p0, p1) = (self.pos[i], self.pos[j]);
                if !within(p0.min(p1), p0.max(p1), o.min, o.max, r) {
                    continue;
                }
                let (e_lo, e_hi) = (p0.min(p1), p0.max(p1));
                if planes_near(e_lo, e_hi, &o, r) < 2 {
                    continue;
                }
                for (k, (ea, eb)) in closest::BOX_EDGES.into_iter().enumerate() {
                    let (b_lo, b_hi) = o.edge_bounds[k];
                    if !within(e_lo, e_hi, b_lo, b_hi, r) {
                        continue;
                    }
                    let (q0, q1) = (o.corners[ea], o.corners[eb]);
                    let (s, t, x, y) = closest::closest_segments(p0, p1, q0, q1);
                    let d = (x - y).length();
                    if d >= r
                        || d <= 1e-6
                        || !(1e-4..1.0 - 1e-4).contains(&s)
                        || !(1e-4..1.0 - 1e-4).contains(&t)
                    {
                        continue;
                    }
                    let n = (x - y) / d;
                    let (wi, wj) = (weight(self, i) * (1.0 - s), weight(self, j) * s);
                    let denom = wi * (1.0 - s) + wj * s;
                    if denom <= 1e-8 {
                        continue;
                    }
                    let k = (r - d) / denom;
                    self.pos[i] += n * (k * wi);
                    self.pos[j] += n * (k * wj);
                }
            }
        }
    }

    /// Bodies push the sheet aside: vertices out of the hull, and edges off its axis so
    /// a thin limb cannot slip between two vertices.
    fn contact_bodies(&mut self, bodies: &[(Vec3, f32, f32)]) {
        for &(feet, height, radius) in bodies {
            let top = feet + Vec3::Y * height;
            for i in 0..self.pos.len() {
                if self.pinned(i) {
                    continue;
                }
                if let Some(out) = verlet::push_out_of_capsule(self.pos[i], feet, top, radius) {
                    self.pos[i] = out;
                }
            }
            for e in 0..self.edges.len() {
                let [i, j] = self.edges[e];
                let (s, _, x, y) = closest::closest_segments(self.pos[i], self.pos[j], feet, top);
                let d = (x - y).length();
                if d >= radius || d <= 1e-6 || !(1e-4..1.0 - 1e-4).contains(&s) {
                    continue;
                }
                let n = (x - y) / d;
                let wi = if self.pinned(i) { 0.0 } else { 1.0 - s };
                let wj = if self.pinned(j) { 0.0 } else { s };
                let denom = wi * (1.0 - s) + wj * s;
                if denom <= 1e-8 {
                    continue;
                }
                let k = (radius - d) / denom;
                self.pos[i] += n * (k * wi);
                self.pos[j] += n * (k * wj);
            }
        }
    }

    /// Wind acceleration per point from the pressure on each triangle: mostly along the
    /// face normal, a little along the fabric. Gusts drift per cloth so neighbouring
    /// flags never beat in step.
    fn wind_accel(&self, time: f32, wind: [f32; 2], h: f32, accel: &mut Vec<Vec3>) {
        let def = self.def;
        accel.clear();
        accel.resize(self.pos.len(), Vec3::ZERO);
        let base = Vec3::new(wind[0], 0.0, wind[1]);
        if base.length_squared() < 1e-8 || def.wind <= 0.0 {
            return;
        }
        let mass = def.size[0] * def.size[1] / self.pos.len() as f32;
        let t = time + self.phase;
        let (cols, rows) = (self.cols(), self.rows());
        let [du, dv] = [
            def.size[0] / f32::from(def.segments[0]),
            def.size[1] / f32::from(def.segments[1]),
        ];
        // Gusts ripple from the post toward the free edge, with a faster flutter down
        // the sheet; separable so a step costs a sine per column and per row.
        let mut ripple = [0.0f32; MAX_GRID];
        let mut flutter = [0.0f32; MAX_GRID];
        for (u, g) in ripple.iter_mut().enumerate().take(cols) {
            *g = 0.3 * (1.7 * t - 2.2 * u as f32 * du).sin();
        }
        for (v, g) in flutter.iter_mut().enumerate().take(rows) {
            *g = 0.15 * (4.1 * t - 2.3 * v as f32 * dv).sin();
        }
        for &[a, b, c] in &self.tris {
            let (pa, pb, pc) = (self.pos[a], self.pos[b], self.pos[c]);
            let cross = (pb - pa).cross(pc - pa);
            let len = cross.length();
            if len < 1e-7 {
                continue;
            }
            let n = cross / len;
            let area = len * 0.5;
            let gust = 1.0 + ripple[a % cols] + flutter[a / cols];
            let vel = ((pa - self.old[a]) + (pb - self.old[b]) + (pc - self.old[c])) / (3.0 * h);
            let rel = base * gust - vel;
            let along = n.dot(rel);
            let force =
                (n * (AERO_NORMAL * along) + (rel - n * along) * AERO_TANGENT) * (area * def.wind);
            let per = force / (3.0 * mass);
            accel[a] += per;
            accel[b] += per;
            accel[c] += per;
        }
    }

    /// Points between the last two steps, `alpha` of the way.
    pub(super) fn points(&self, alpha: f32) -> impl Iterator<Item = Vec3> + '_ {
        self.prev
            .iter()
            .zip(&self.pos)
            .map(move |(a, b)| a.lerp(*b, alpha))
    }

    /// The box every point lies in, relative to the owning cell.
    pub(super) fn reach() -> (Vec3, Vec3) {
        (Vec3::splat(-MAX_EXTENT), Vec3::splat(1.0 + MAX_EXTENT))
    }
}

/// A `cols × rows` grid's triangles, wound the way the renderer draws them.
fn triangles(cols: usize, rows: usize) -> impl Iterator<Item = [usize; 3]> {
    (0..rows - 1).flat_map(move |v| {
        (0..cols - 1).flat_map(move |u| {
            let i = v * cols + u;
            [[i, i + 1, i + cols], [i + 1, i + cols + 1, i + cols]]
        })
    })
}

/// The triangles' edges: rows, columns and each quad's split diagonal.
fn edges(cols: usize, rows: usize) -> impl Iterator<Item = [usize; 2]> {
    (0..rows).flat_map(move |v| {
        (0..cols).flat_map(move |u| {
            let i = v * cols + u;
            let right = (u + 1 < cols).then_some([i, i + 1]);
            let down = (v + 1 < rows).then_some([i, i + cols]);
            let diag = (u + 1 < cols && v + 1 < rows).then_some([i + 1, i + cols]);
            [right, down, diag].into_iter().flatten()
        })
    })
}

/// Whether two boxes come within `r` of each other along every axis: a cheap superset
/// of "within distance `r`", for culling before an exact query.
#[inline]
fn within(lo: Vec3, hi: Vec3, b_lo: Vec3, b_hi: Vec3, r: f32) -> bool {
    (lo - b_hi).cmple(Vec3::splat(r)).all() && (b_lo - hi).cmple(Vec3::splat(r)).all()
}

/// On how many axes the bounds `lo..hi` come within `r` of one of the box's two face
/// planes. A box edge lies on planes of two axes and a corner on three, so a primitive
/// near fewer cannot be near any of them.
#[inline]
fn planes_near(lo: Vec3, hi: Vec3, o: &Obstacle, r: f32) -> u32 {
    (0..3)
        .filter(|&k| {
            [o.min[k], o.max[k]]
                .iter()
                .any(|&plane| lo[k] - r <= plane && plane <= hi[k] + r)
        })
        .count() as u32
}

fn is_full_cube(boxes: &[Aabb]) -> bool {
    matches!(boxes, [b] if b.min == [0.0; 3] && b.max == [1.0; 3])
}

/// A per-cell phase so neighbouring flags never move in step.
fn phase_of(cell: IVec3) -> f32 {
    let h = (cell.x as u32)
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add((cell.y as u32).wrapping_mul(0x85EB_CA6B))
        .wrapping_add((cell.z as u32).wrapping_mul(0xC2B2_AE35));
    (h >> 8) as f32 / (1u32 << 24) as f32 * std::f32::consts::TAU
}

/// The sheet posed without simulation, on a `segments` grid relative to `cell`: it
/// streams downwind, drooping more the calmer the air (fitted to the sim's settled
/// shapes), with a ripple running from the post to the free edge. Drawn beyond
/// simulation range and used to start a sim, so neither handover pops.
pub(super) fn far_pose(
    def: &ClothDef,
    cell: IVec3,
    wind: [f32; 2],
    time: f32,
    segments: [usize; 2],
    out: &mut Vec<Vec3>,
) {
    let speed = Vec3::new(wind[0], 0.0, wind[1]).length() * def.wind;
    let downwind = Vec3::new(wind[0], 0.0, wind[1]).normalize_or(Vec3::NEG_X);
    let across = Vec3::Y.cross(downwind);
    let droop =
        FAR_DROOP_MIN + (FAR_DROOP_MAX - FAR_DROOP_MIN) * (-speed / FAR_DROOP_FALLOFF).exp();
    let (sin, cos) = droop.sin_cos();
    let amplitude = FAR_RIPPLE * (speed / 1.5).min(2.0);
    let omega = 3.0 + 1.5 * speed;
    let t = time + phase_of(cell);
    let anchor = Vec3::from_array(def.anchor);
    let [cols, rows] = segments.map(|s| s.max(1));
    let [width, height] = def.size;
    out.clear();
    for v in 0..=rows {
        for u in 0..=cols {
            let along = u as f32 / cols as f32;
            let x = along * width;
            let y = v as f32 / rows as f32 * height;
            let wave = amplitude * along * (FAR_WAVE_NUMBER * x - omega * t).sin();
            out.push(anchor + downwind * (x * cos) - Vec3::Y * (y + x * sin) + across * wave);
        }
    }
}

/// One grid constraint from point `i` to a later point `j`; a pinned end never moves.
#[inline]
fn link(pos: &mut [Vec3], i: usize, j: usize, rest: f32, pinned: [bool; 2], k: f32) {
    let (lo, hi) = pos.split_at_mut(j);
    let w = pinned.map(|p| if p { 0.0 } else { 1.0 });
    verlet::satisfy_distance(&mut lo[i], &mut hi[0], rest, w[0], w[1], k);
}

/// Every cloth near the camera, simulated at a fixed step.
#[derive(Default)]
pub(super) struct ClothSystem {
    sims: FxHashMap<IVec3, ClothSim>,
    placed: Vec<PlacedCloth>,
    hulls: Vec<BodyHull>,
    near: Vec<(Vec3, f32, f32)>,
    acc: f32,
    time: f32,
    wind: [f32; 2],
}

impl ClothSystem {
    pub(super) fn tick(
        &mut self,
        world: &ReplicaWorld,
        camera: WorldPos,
        wind: [f32; 2],
        bodies: impl IntoIterator<Item = BodyHull>,
        dt: f32,
    ) {
        self.hulls.clear();
        self.hulls.extend(bodies);
        self.wind = wind;
        let center = camera.block();
        let dist2 = |p: &PlacedCloth| (p.cell - center).as_i64vec3().length_squared();
        world.collect_cloths(center, SIM_RADIUS + SIM_HYSTERESIS, &mut self.placed);
        self.placed.sort_by_key(dist2);
        self.placed.truncate(MAX_ACTIVE);
        let placed = &self.placed;
        self.sims.retain(|cell, sim| {
            placed
                .iter()
                .any(|p| p.cell == *cell && p.cloth == sim.def.id)
        });
        let entry_radius2 = i64::from(SIM_RADIUS) * i64::from(SIM_RADIUS);
        for p in placed.iter().filter(|p| dist2(p) <= entry_radius2) {
            if let (std::collections::hash_map::Entry::Vacant(slot), Some(def)) = (
                self.sims.entry(p.cell),
                petramond_world::cloth::def(p.cloth),
            ) {
                slot.insert(ClothSim::new(p.cell, def, wind, self.time));
            }
        }

        self.acc = (self.acc + dt.max(0.0)).min(STEP * MAX_STEPS_PER_FRAME as f32);
        let data = world.data();
        while self.acc >= STEP {
            self.acc -= STEP;
            self.time += STEP;
            for sim in self.sims.values_mut() {
                let cell = sim.cell;
                let boxes = |c: IVec3| {
                    let w = cell + c;
                    data.collision_boxes_at(w.x, w.y, w.z)
                };
                self.near.clear();
                let (lo, hi) = ClothSim::reach();
                for b in &self.hulls {
                    let feet = b.feet.relative_to(cell);
                    let reach = Vec3::new(b.radius, b.height, b.radius);
                    if (feet + reach).cmpge(lo).all() && (feet - reach).cmple(hi).all() {
                        self.near.push((feet, b.height, b.radius + BODY_MARGIN));
                    }
                }
                sim.step(self.time, wind, &boxes, &self.near);
            }
        }
    }

    pub(super) fn alpha(&self) -> f32 {
        (self.acc / STEP).clamp(0.0, 1.0)
    }

    pub(super) fn sims(&self) -> impl Iterator<Item = &ClothSim> {
        self.sims.values()
    }

    pub(super) fn simulates(&self, cell: IVec3) -> bool {
        self.sims.contains_key(&cell)
    }

    /// The clock and wind far poses are drawn at, continuous with the sims.
    pub(super) fn far_clock(&self) -> (f32, [f32; 2]) {
        (self.time + self.acc, self.wind)
    }

    /// The grid a far flag at `distance` blocks is drawn with.
    pub(super) fn far_segments(def: &ClothDef, distance: f32) -> [usize; 2] {
        let shift = if distance > FAR_QUARTER_GRID {
            2
        } else if distance > FAR_HALF_GRID {
            1
        } else {
            0
        };
        def.segments.map(|s| (usize::from(s) >> shift).max(1))
    }

    pub(super) fn clear(&mut self) {
        self.sims.clear();
        self.acc = 0.0;
    }
}

impl super::Game {
    pub(super) fn tick_cloth(&mut self, dt: f32) {
        let wind = self.client_mod_cloth_wind().unwrap_or(DEFAULT_WIND);
        let camera = self.render_camera().pos;
        // A long body (a cow, a cart) gets a hull as wide as it is long: cloth may stand
        // off its sides a little, but never sinks into its flanks.
        let bodies = self.presented_entities_cache.iter().map(|e| BodyHull {
            feet: WorldPos::new(e.feet[0], e.feet[1], e.feet[2]),
            height: e.size[1],
            radius: e.mob_kind.map_or(e.size[0] * 0.5, |kind| {
                let size = petramond::mob::def(petramond::mob::Mob(kind.0)).size;
                size.half_length
                    .unwrap_or(size.half_width)
                    .max(size.half_width)
            }),
        });
        self.fx
            .cloth
            .tick(&self.replica.world, camera, wind, bodies, dt);
    }
}

#[cfg(test)]
mod tests;
