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
//! vs box, box corner vs cloth triangle or edge, box edge vs cloth edge). Contacts are
//! found on the clear positions a substep starts from, so each knows which side of its
//! box the cloth is on. Every triangle near a box gets the plane separating them, and
//! no vertex may close more than `BOUND_RELAX` of that gap per substep: the sheet slides
//! along and away from blocks freely but never passes through them between its
//! vertices, with no continuous collision detection.
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
use rustc_hash::{FxHashMap, FxHashSet};

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
/// How near a box, on the clear positions, a triangle must be to look for contacts:
/// from farther it closes only [`BOUND_RELAX`] of the gap in a substep, so it ends
/// beyond [`CONTACT_RADIUS`] anyway.
const CONTACT_REACH: f32 = CONTACT_RADIUS * 2.0;
const _: () = assert!(CONTACT_REACH * (1.0 - BOUND_RELAX) > CONTACT_RADIUS);
/// How far a triangle looks for boxes when bounding its vertices' moves; far from
/// everything a vertex may move this much times [`BOUND_RELAX`] per substep.
const QUERY_RADIUS: f32 = 0.25;
/// Fraction of its gap to a box a triangle may close per substep, so a sheet slows as
/// it nears a block instead of landing on it.
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

/// A triangle's separating plane from one box, found on the clear positions: each of
/// its vertices may move at most `slack` against `n` this substep, and freely along it.
#[derive(Copy, Clone, Debug)]
struct Bound {
    n: Vec3,
    slack: f32,
}

/// A contact found on the clear positions: the point `weights` blend from `points` must
/// stay [`CONTACT_RADIUS`] out along `n` from the box's supporting plane at `at`.
#[derive(Copy, Clone, Debug)]
struct Contact {
    points: [usize; 3],
    weights: [f32; 3],
    n: Vec3,
    at: f32,
}

impl Contact {
    fn new(points: [usize; 3], weights: [f32; 3], n: Vec3, on_box: Vec3) -> Self {
        Self {
            points,
            weights,
            n,
            at: n.dot(on_box),
        }
    }
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
    /// Each triangle's bounds this substep: `bounds[tri_bounds[t]..tri_bounds[t + 1]]`.
    bounds: Vec<Bound>,
    tri_bounds: Vec<u32>,
    contacts: Vec<Contact>,
    obstacles: Vec<Obstacle>,
    /// The obstacles within [`QUERY_RADIUS`] of the whole sheet this substep.
    nearby: Vec<u32>,
    full: Vec<bool>,
    tris: Box<[[usize; 3]]>,
    edges: Box<[[usize; 2]]>,
    /// The triangles around each vertex: `vertex_tris[around[i]..around[i + 1]]`.
    around: Box<[u32]>,
    vertex_tris: Box<[u32]>,
    /// Per triangle, the vertices and edges it holds first; see [`first_holds`].
    holds: Box<[u8]>,
    phase: f32,
    #[cfg(test)]
    furls: u32,
}

impl ClothSim {
    /// A sim starting from the far pose at `time`, so a flag coming into simulation
    /// range carries on from the shape it was just drawn in.
    pub(super) fn new(cell: IVec3, def: &'static ClothDef, wind: [f32; 2], time: f32) -> Self {
        let [cols, rows] = def.segments.map(|s| usize::from(s) + 1);
        let tris: Box<[[usize; 3]]> = triangles(cols, rows).collect();
        let (around, vertex_tris) = triangles_around(&tris, cols * rows);
        let holds = first_holds(&tris, cols * rows);
        let mut sim = Self {
            cell,
            def,
            pos: Vec::new(),
            old: Vec::new(),
            prev: Vec::new(),
            accel: Vec::new(),
            clear: Vec::new(),
            bounds: Vec::new(),
            tri_bounds: Vec::new(),
            contacts: Vec::new(),
            obstacles: Vec::new(),
            nearby: Vec::new(),
            full: Vec::new(),
            tris,
            edges: edges(cols, rows).collect(),
            around,
            vertex_tris,
            holds,
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
            if !self.survey() {
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
            // Blocks have the last word: fabric squeezed between a body and a block
            // stays off the block and the body sinks into it instead.
            self.contact_bodies(bodies);
            self.contact_boxes();
            for i in 0..self.pos.len() {
                if self.pinned(i) {
                    self.pos[i] = self.pin(i);
                    self.old[i] = self.pos[i];
                    continue;
                }
                self.pos[i] = self.clear[i] + self.allowed_move(i, self.pos[i] - self.clear[i]);
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

    /// Surveys the clear positions a substep starts from: each triangle's separating
    /// plane from every box near it, with how much of the gap its vertices may close,
    /// and every contact the sheet could close (see [`find_contacts`]). False when a
    /// triangle already overlaps a box, or touches one with no side to part along.
    fn survey(&mut self) -> bool {
        self.bounds.clear();
        self.tri_bounds.clear();
        self.tri_bounds.push(0);
        self.contacts.clear();
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for p in &self.pos {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        self.nearby.clear();
        self.nearby
            .extend((0..self.obstacles.len() as u32).filter(|&k| {
                let o = &self.obstacles[k as usize];
                closest::box_box_distance(lo, hi, o.min, o.max) < QUERY_RADIUS
            }));
        if self.nearby.is_empty() {
            self.tri_bounds.resize(self.tris.len() + 1, 0);
            return true;
        }
        let cols = self.cols();
        for t in 0..self.tris.len() {
            let [a, b, c] = self.tris[t];
            let (pa, pb, pc) = (self.pos[a], self.pos[b], self.pos[c]);
            let (lo, hi) = (pa.min(pb).min(pc), pa.max(pb).max(pc));
            for &k in &self.nearby {
                let o = &self.obstacles[k as usize];
                let apart = (lo - o.max).max(Vec3::ZERO) - (o.min - hi).max(Vec3::ZERO);
                let gap = apart.length();
                if gap >= QUERY_RADIUS {
                    continue;
                }
                if gap < CONTACT_REACH {
                    find_contacts(
                        o,
                        self.tris[t],
                        self.holds[t],
                        &self.pos,
                        cols,
                        &mut self.contacts,
                    );
                }
                // Far enough out, the bounds' own separating direction parts them by at
                // least their gap; closer, as where a triangle slants across a corner,
                // only the exact nearest pair leaves room to move.
                let (n, gap) = if gap >= CONTACT_RADIUS * 0.5 {
                    (apart / gap, gap)
                } else {
                    let nearest = closest::triangle_box_closest(pa, pb, pc, o.min, o.max)
                        .map_or(Vec3::ZERO, |(x, y)| x - y);
                    closest::triangle_box_separation(pa, pb, pc, o.min, o.max, nearest)
                };
                if gap <= 0.0 {
                    if self.pinned(a) && self.pinned(b) && self.pinned(c) {
                        continue;
                    }
                    return false;
                }
                self.bounds.push(Bound {
                    n,
                    slack: BOUND_RELAX * (gap - BOUND_SLACK).max(0.0),
                });
            }
            self.tri_bounds.push(self.bounds.len() as u32);
        }
        true
    }

    fn bounds_around(&self, i: usize) -> impl Iterator<Item = &Bound> {
        let tris = &self.vertex_tris[self.around[i] as usize..self.around[i + 1] as usize];
        tris.iter().flat_map(|&t| {
            let t = t as usize;
            &self.bounds[self.tri_bounds[t] as usize..self.tri_bounds[t + 1] as usize]
        })
    }

    /// `want` cut to what vertex `i`'s bounds allow: capped for boxes beyond the query,
    /// slid along any plane it would close too far on, then shortened if sliding along
    /// one plane still crosses another. Any move along or away from a box stays allowed,
    /// so fabric that has come to rest against a block never sticks to it. Sliding never
    /// lengthens a move (standing still is always allowed), so the cap still holds.
    fn allowed_move(&self, i: usize, mut want: Vec3) -> Vec3 {
        let cap = BOUND_RELAX * QUERY_RADIUS;
        let len = want.length();
        if len > cap {
            want *= cap / len;
        }
        if self.bounds.is_empty() {
            return want;
        }
        for _ in 0..2 {
            let mut slid = false;
            for b in self.bounds_around(i) {
                let along = b.n.dot(want);
                if along < -b.slack {
                    want += b.n * (-b.slack - along);
                    slid = true;
                }
            }
            if !slid {
                return want;
            }
        }
        let keep = self.bounds_around(i).fold(1.0f32, |keep, b| {
            let along = b.n.dot(want);
            if along < -b.slack {
                keep.min(b.slack / -along)
            } else {
                keep
            }
        });
        want * keep
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

    /// Projects each contact's point back out to [`CONTACT_RADIUS`] from its plane,
    /// split over the points it blends by their weights.
    fn contact_boxes(&mut self) {
        for k in 0..self.contacts.len() {
            let Contact {
                points,
                weights,
                n,
                at,
            } = self.contacts[k];
            let p: Vec3 = (0..3).map(|j| self.pos[points[j]] * weights[j]).sum();
            let gap = n.dot(p) - at;
            if gap >= CONTACT_RADIUS {
                continue;
            }
            let w = std::array::from_fn::<f32, 3, _>(|j| {
                if self.pinned(points[j]) {
                    0.0
                } else {
                    weights[j]
                }
            });
            let denom: f32 = (0..3).map(|j| w[j] * weights[j]).sum();
            if denom <= 1e-8 {
                continue;
            }
            let push = (CONTACT_RADIUS - gap) / denom;
            for j in 0..3 {
                self.pos[points[j]] += n * (push * w[j]);
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

/// Each vertex's triangles, flattened: `(around, list)`, vertex `i`'s being
/// `list[around[i]..around[i + 1]]`.
fn triangles_around(tris: &[[usize; 3]], points: usize) -> (Box<[u32]>, Box<[u32]>) {
    let mut around = vec![0u32; points + 1];
    for &i in tris.iter().flatten() {
        around[i + 1] += 1;
    }
    for i in 0..points {
        around[i + 1] += around[i];
    }
    let mut fill = around.clone();
    let mut list = vec![0u32; tris.len() * 3];
    for (t, tri) in tris.iter().enumerate() {
        for &i in tri {
            list[fill[i] as usize] = t as u32;
            fill[i] += 1;
        }
    }
    (around.into(), list.into())
}

/// Which of each triangle's vertices (bits 0..3) and edges (bits 3..6: `ab`, `bc`,
/// `ca`) no earlier triangle holds. A feature within reach of a box puts every triangle
/// holding it within reach too, so contacts found only through the first holder are
/// found once each.
fn first_holds(tris: &[[usize; 3]], points: usize) -> Box<[u8]> {
    let mut seen = vec![false; points];
    let mut seen_edges = FxHashSet::default();
    tris.iter()
        .map(|&[a, b, c]| {
            let mut holds = 0;
            for (k, i) in [a, b, c].into_iter().enumerate() {
                if !std::mem::replace(&mut seen[i], true) {
                    holds |= 1 << k;
                }
            }
            for (k, (i, j)) in [(a, b), (b, c), (c, a)].into_iter().enumerate() {
                if seen_edges.insert((i.min(j), i.max(j))) {
                    holds |= 8 << k;
                }
            }
            holds
        })
        .collect()
}

/// Every contact triangle `tri` brings within [`CONTACT_REACH`] of box `o` on the clear
/// positions, through the pairings two convex shapes meet at: cloth vertex vs box, box
/// corner vs cloth triangle, box edge or corner vs cloth edge; vertices and edges only
/// where `holds` says this triangle holds them first. A pair counts only where the box's
/// nearest point to the cloth is that feature, so each contact's plane supports the
/// whole box, and knows the side the cloth is on even after the links drag a point into
/// a thin post.
fn find_contacts(
    o: &Obstacle,
    tri: [usize; 3],
    holds: u8,
    pos: &[Vec3],
    cols: usize,
    out: &mut Vec<Contact>,
) {
    let reach = CONTACT_REACH;
    let p = tri.map(|i| pos[i]);
    for k in 0..3 {
        if holds & (1 << k) == 0 || tri[k].is_multiple_of(cols) {
            continue;
        }
        let q = closest::closest_on_box(p[k], o.min, o.max);
        let d = (p[k] - q).length();
        if d < reach && d > 1e-6 {
            out.push(Contact::new(
                [tri[k]; 3],
                [1.0, 0.0, 0.0],
                (p[k] - q) / d,
                q,
            ));
        }
    }
    let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
    if planes_near(lo, hi, o, reach) >= 3 {
        for q in o.corners {
            if !within(lo, hi, q, q, reach) {
                continue;
            }
            let (pt, bary) = closest::closest_on_triangle(q, p[0], p[1], p[2]);
            let d = (pt - q).length();
            if d >= reach
                || d <= 1e-6
                || bary.iter().any(|&x| x <= 1e-4)
                || pt.clamp(o.min, o.max) != q
            {
                continue;
            }
            out.push(Contact::new(tri, bary, (pt - q) / d, q));
        }
    }
    for (k, (i, j)) in [(0, 1), (1, 2), (2, 0)].into_iter().enumerate() {
        if holds & (8 << k) == 0 {
            continue;
        }
        let (p0, p1) = (p[i], p[j]);
        let (lo, hi) = (p0.min(p1), p0.max(p1));
        if !within(lo, hi, o.min, o.max, reach) || planes_near(lo, hi, o, reach) < 2 {
            continue;
        }
        for (e, (ea, eb)) in closest::BOX_EDGES.into_iter().enumerate() {
            let (b_lo, b_hi) = o.edge_bounds[e];
            if !within(lo, hi, b_lo, b_hi, reach) {
                continue;
            }
            let (s, _, x, y) = closest::closest_segments(p0, p1, o.corners[ea], o.corners[eb]);
            let d = (x - y).length();
            if d >= reach
                || d <= 1e-6
                || !(1e-4..1.0 - 1e-4).contains(&s)
                || (x.clamp(o.min, o.max) - y).length() > 1e-5
            {
                continue;
            }
            out.push(Contact::new(
                [tri[i], tri[j], tri[j]],
                [1.0 - s, s, 0.0],
                (x - y) / d,
                y,
            ));
        }
    }
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
