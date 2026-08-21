//! Foliage placers — build a tree's leaves around the trunk's attach points.
//!
//! Each placer is a *family* of canopy shape (conifer cone, droopy swamp, flat
//! umbrella). The per-tree differences — width, raggedness, drip chance — are
//! FIELDS on the placer, so a new look is a data row in `data::features`, not a
//! new impl. A genuinely new *shape* (different layer profile, entangled
//! branches) is a new placer or a bespoke `Feature` instead. Broadleaf trees
//! (oak/birch/jungle) no longer use a placer pair at all — they
//! are `tree::CanopyTreeFeature` skeletons.
//!
//! Iteration order and the per-cell RNG draws are fixed so cross-chunk seam
//! replay stays deterministic (a tree rooted in a neighbour materialises
//! identically here). Parameterising never reorders or adds draws: each field
//! simply names a constant the loop already consumed.
//!
//! Placement is collect-then-commit through [`Canopy`]: the loops record
//! candidate cells (consuming their draws exactly as before), and the commit
//! places only candidates that hold a face-connected path to the trunk's wood
//! through cells the caller's `open` oracle allows. That is what keeps a
//! worldgen canopy permanent: no leaf inside terrain or past a cave wall, and
//! no leaf the decay flood would find cut off from its logs.

use std::collections::VecDeque;

use crate::feature::placers::trunk::TrunkPlan;
use crate::feature::FeatureCtx;
use crate::rng::FeatureRng;
use petramond_world::block::behavior::MAX_LOG_DISTANCE;
use petramond_world::block::Block;
use petramond_world::mathh::{IVec3, FACE_NEIGHBORS};

pub trait FoliagePlacer: Send + Sync {
    /// Build the canopy. `open` answers whether a world cell may hold a canopy
    /// leaf or route leaf-support through one (see [`Canopy::commit`]); the
    /// trunk plan carries the attach point(s) and the support logs.
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        trunk: &TrunkPlan,
        leaf: Block,
        rng: &mut FeatureRng,
    );

    /// Horizontal Chebyshev reach of any leaf cell from the attach column.
    /// `data::features` validates `reach + trunk lean` against the candidate
    /// window fence at load, so the `open` oracle's reads always stay inside
    /// the window every replaying chunk can serve.
    fn horizontal_reach(&self) -> i32;
}

/// Candidate canopy cells, recorded in draw order and committed connectivity-
/// checked.
///
/// Collection and commit are split so the RNG draw sequence stays exactly the
/// per-cell order the loops always consumed, while the WRITE set becomes a
/// pure function of (draws, `open`) — both world-anchored, so every chunk
/// replaying the tree keeps the same cells and the sink clips the rest. A
/// candidate is placed iff a face-step path of open candidates links it to a
/// trunk log within [`MAX_LOG_DISTANCE`] steps (the exact reach the leaf-decay
/// flood enforces). Cells `open` refuses — inside a hillside, past a cave
/// wall — are dropped, and so is anything only they would have connected:
/// a canopy never leaks through solid ground into a cave behind it, and never
/// places a leaf that would decay after generation.
pub(crate) struct Canopy {
    cells: Vec<IVec3>,
}

impl Canopy {
    pub(crate) fn new() -> Self {
        Self { cells: Vec::new() }
    }

    pub(crate) fn add(&mut self, p: IVec3) {
        self.cells.push(p);
    }

    pub(crate) fn commit(
        self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        logs: &[IVec3],
        leaf: Block,
    ) {
        if self.cells.is_empty() {
            return;
        }
        const CANDIDATE: u8 = 1;
        const OPEN: u8 = 2;
        const WOOD: u8 = 4;
        const KEEP: u8 = 8;

        // One dense scratch grid over the canopy ∪ trunk bounding box.
        let mut lo = self.cells[0];
        let mut hi = self.cells[0];
        for &p in self.cells.iter().chain(logs) {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let size = hi - lo + IVec3::ONE;
        let (sx, sy) = (size.x as usize, size.y as usize);
        let idx = |p: IVec3| {
            ((p.z - lo.z) as usize * sy + (p.y - lo.y) as usize) * sx + (p.x - lo.x) as usize
        };
        let mut grid = vec![0u8; sx * sy * size.z as usize];

        // The oracle is consulted once per unique candidate, in draw order.
        for &p in &self.cells {
            let i = idx(p);
            if grid[i] & CANDIDATE == 0 {
                grid[i] |= CANDIDATE;
                if open(p) {
                    grid[i] |= OPEN;
                }
            }
        }

        // Flood from the trunk's wood through open candidates, bounded by the
        // decay flood's log distance so nothing kept can ever rot.
        let mut frontier: VecDeque<(IVec3, i32)> = VecDeque::new();
        for &l in logs {
            grid[idx(l)] |= WOOD;
            frontier.push_back((l, 0));
        }
        while let Some((p, d)) = frontier.pop_front() {
            if d >= MAX_LOG_DISTANCE {
                continue;
            }
            for step in FACE_NEIGHBORS {
                let n = p + step;
                if n.cmplt(lo).any() || n.cmpgt(hi).any() {
                    continue;
                }
                let i = idx(n);
                if grid[i] & (CANDIDATE | OPEN) == CANDIDATE | OPEN && grid[i] & (WOOD | KEEP) == 0
                {
                    grid[i] |= KEEP;
                    frontier.push_back((n, d + 1));
                }
            }
        }

        for &p in &self.cells {
            if grid[idx(p)] & KEEP != 0 {
                ctx.set_leaf(p, leaf);
            }
        }
    }
}

/// Collect a square leaf layer of the given radius at world Y `y`, centred on
/// `(cx, cz)`. Hard corners (|lx|==|lz|==radius) are always cut; outer-ring cells
/// are trimmed with probability `ragged` for a natural edge — EXCEPT the four
/// cells face-adjacent to the centre column. On a radius-1 layer the outer
/// ring IS the trunk-hugging ring, and a trimmed cell there reads as a block
/// missing from the tree, not as a ragged silhouette (playtest 2026-08-20) —
/// the same "never bald against the trunk" rule the conifer crown rings state.
/// The trim draw is still consumed for those cells, so no other cell's
/// outcome moves.
fn leaf_layer(
    canopy: &mut Canopy,
    cx: i32,
    y: i32,
    cz: i32,
    radius: i32,
    ragged: f32,
    rng: &mut FeatureRng,
) {
    for lx in -radius..=radius {
        for lz in -radius..=radius {
            if lx.abs() == radius && lz.abs() == radius {
                continue; // cut hard corners
            }
            let outer = lx.abs() == radius || lz.abs() == radius;
            let hugs_centre = lx.abs() + lz.abs() == 1;
            if outer && ragged > 0.0 && rng.chance(ragged) && !hugs_centre {
                continue; // ragged edge
            }
            canopy.add(IVec3::new(cx + lx, y, cz + lz));
        }
    }
}

/// Collect the four cardinal-neighbour leaves around `(cx, y, cz)` — a
/// deterministic '+' with no ragged trimming (every face is always filled). The
/// centre cell is left to the trunk log, which the commit's `set_leaf` keeps.
fn plus_ring(canopy: &mut Canopy, cx: i32, y: i32, cz: i32) {
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        canopy.add(IVec3::new(cx + dx, y, cz + dz));
    }
}

/// Droopy swamp canopy: a wide flat main layer at the trunk top, a small cap one
/// block above (`radius - 1`), and leaves that hang one block down from the outer
/// ring (the swamp "drip"). `ragged` trims the main layer's edge; `drip_skip` is
/// the chance to omit each individual hanging drip. A drip whose ring cell above
/// was trimmed away has no face-connected path back to the trunk and is dropped
/// by the commit — it would only have rotted.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroopyFoliage {
    pub radius: i32,
    pub ragged: f32,
    pub drip_skip: f32,
}

impl FoliagePlacer for DroopyFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        trunk: &TrunkPlan,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = trunk.attach[0];
        let (cx, cz, ct) = (a.x, a.z, a.y);
        let r = self.radius;
        let mut canopy = Canopy::new();
        // Wide main layer + a small cap.
        leaf_layer(&mut canopy, cx, ct, cz, r, self.ragged, rng);
        leaf_layer(&mut canopy, cx, ct + 1, cz, r - 1, 0.0, rng);
        // Hanging drips: from each outer-ring cell of the main layer, sometimes
        // extend one leaf straight down.
        for lx in -r..=r {
            for lz in -r..=r {
                if lx.abs() == r && lz.abs() == r {
                    continue;
                }
                if !(lx.abs() == r || lz.abs() == r) {
                    continue; // outer ring only
                }
                if rng.chance(self.drip_skip) {
                    continue;
                }
                canopy.add(IVec3::new(cx + lx, ct - 1, cz + lz));
            }
        }
        canopy.commit(ctx, open, &trunk.logs, leaf);
    }

    fn horizontal_reach(&self) -> i32 {
        self.radius
    }
}

/// Conifer canopy (spruce / pine): a deterministic pointed top — a single-leaf
/// tip, a '+'-crown hugging the top log, and a second '+' on the third block
/// down (all four faces always filled) — over widening ragged "skirts" that give
/// the canonical drooping evergreen silhouette. `radius` controls how wide/tall the
/// skirts grow (clamped to ≥2 so the pointed top stays intact); `skirt_ragged`
/// is the outer-ring trim chance per skirt.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConiferFoliage {
    pub radius: i32,
    pub skirt_ragged: f32,
}

impl FoliagePlacer for ConiferFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        trunk: &TrunkPlan,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = trunk.attach[0];
        let max_r = self.radius.max(2);
        let mut canopy = Canopy::new();

        // Deterministic pointed top, so a spruce is never bald or lopsided up
        // there: a single-leaf tip, a '+'-crown around the top log, and a second
        // '+' on the third block down. Both rings are ALWAYS four full faces (no
        // ragged trimming); their centres are trunk logs the commit keeps.
        canopy.add(IVec3::new(a.x, a.y + 1, a.z)); // tip
        plus_ring(&mut canopy, a.x, a.y, a.z); // crown (1st block from top)
        plus_ring(&mut canopy, a.x, a.y - 2, a.z); // 3rd block from top

        // Widening ragged skirts below the top three blocks: a two-step
        // wide/narrow cycle for the drooping conifer silhouette. The top is
        // placed above, so descent starts at the first skirt. Iteration and
        // per-cell draw order are fixed for deterministic cross-chunk replay.
        let layers = 4 + max_r * 2;
        for i in 4..layers {
            let y = a.y - i;
            let grow = (i / 2).min(max_r);
            let r = if i % 2 == 1 { (grow - 1).max(0) } else { grow };
            leaf_layer(&mut canopy, a.x, y, a.z, r, self.skirt_ragged, rng);
        }

        canopy.commit(ctx, open, &trunk.logs, leaf);
    }

    fn horizontal_reach(&self) -> i32 {
        self.radius.max(2)
    }
}

/// Flat sparse savanna canopy (acacia-like silhouette): a thin diamond umbrella
/// spread above a tall trunk, with gaps so it reads as airy. `upper_*` is the
/// raised umbrella disc; `lower_*` is a sparser ring one block below it.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatSparseFoliage {
    pub upper_radius: i32,
    pub upper_skip: f32,
    pub lower_radius: i32,
    pub lower_skip: f32,
}

impl FoliagePlacer for FlatSparseFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        trunk: &TrunkPlan,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = trunk.attach[0];
        let (cx, cz, ct) = (a.x, a.z, a.y);
        let mut canopy = Canopy::new();
        // Upper umbrella: a SOLID diamond, ragged only on its outermost ring. A
        // thin disc with random interior holes leaves leaves attached to the trunk
        // only DIAGONALLY, and the leaf-decay flood travels face-steps only, so
        // those leaves read as cut off and rot. Keeping the interior solid
        // guarantees every leaf has an orthogonal path inward to the centre cell,
        // which sits directly above the top log — the whole canopy stays supported.
        let ur = self.upper_radius;
        for lx in -ur..=ur {
            for lz in -ur..=ur {
                let d = lx.abs() + lz.abs();
                if d > ur {
                    continue;
                }
                if d == ur && rng.chance(self.upper_skip) {
                    continue; // ragged edge only
                }
                canopy.add(IVec3::new(cx + lx, ct + 1, cz + lz));
            }
        }
        // Lower skirt one block down: still sparse for the airy savanna read, but
        // every cell sits directly beneath the solid disc above, so even a holey
        // skirt stays orthogonally connected upward to it.
        let lr = self.lower_radius;
        for lx in -lr..=lr {
            for lz in -lr..=lr {
                if lx.abs() + lz.abs() > lr {
                    continue;
                }
                if rng.chance(self.lower_skip) {
                    continue;
                }
                canopy.add(IVec3::new(cx + lx, ct, cz + lz));
            }
        }
        canopy.commit(ctx, open, &trunk.logs, leaf);
    }

    fn horizontal_reach(&self) -> i32 {
        self.upper_radius.max(self.lower_radius)
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod spruce_tests {
    use super::*;
    use crate::rng::FeatureRng;
    use petramond_world::chunk::Chunk;

    /// Trunk column logs from `base` up `h` blocks, written into `chunk`, as a
    /// [`TrunkPlan`] — what a straight trunk placer would produce.
    fn column_trunk(chunk: &mut Chunk, cx: i32, cz: i32, base: i32, h: i32) -> TrunkPlan {
        let mut logs = Vec::new();
        for i in 0..h {
            chunk.set_block_raw(
                cx as usize,
                (base + i) as usize,
                cz as usize,
                Block::SpruceLog.id(),
            );
            logs.push(IVec3::new(cx, base + i, cz));
        }
        TrunkPlan {
            attach: vec![IVec3::new(cx, base + h - 1, cz)],
            logs,
        }
    }

    /// A spruce must ALWAYS get its full pointed top regardless of the RNG: a
    /// single-leaf tip, a four-face '+'-crown around the top log, and a four-face
    /// '+' on the third block from the top. (Skirts below stay ragged.)
    #[test]
    fn spruce_crown_and_third_block_are_deterministic_plus() {
        const FACES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        for radius in [2, 3] {
            for seed in [1u32, 7, 42, 1000, 31337] {
                let mut chunk = Chunk::new(0, 0);
                let (cx, cz, base, h) = (8i32, 8i32, 64i32, 9i32);
                let plan = column_trunk(&mut chunk, cx, cz, base, h);
                let top = plan.attach[0];
                let mut rng = FeatureRng::positional(seed, 0xABCD, cx, 0, cz);
                let mut sink = crate::feature::ChunkSink::new(&mut chunk);
                let mut ctx = FeatureCtx::new(&mut sink);
                let cone = ConiferFoliage {
                    radius,
                    skirt_ragged: 0.25,
                };
                cone.place(
                    &mut ctx,
                    &mut |_| true,
                    &plan,
                    Block::SpruceLeaves,
                    &mut rng,
                );

                let leaf = |x: i32, y: i32, z: i32| {
                    chunk.block_raw(x as usize, y as usize, z as usize) == Block::SpruceLeaves.id()
                };
                assert!(
                    leaf(cx, top.y + 1, cz),
                    "r{radius} seed {seed}: missing tip"
                );
                for (dx, dz) in FACES {
                    assert!(
                        leaf(cx + dx, top.y, cz + dz),
                        "r{radius} seed {seed}: crown face {dx},{dz}"
                    );
                    assert!(
                        leaf(cx + dx, top.y - 2, cz + dz),
                        "r{radius} seed {seed}: 3rd-block face {dx},{dz}"
                    );
                }
            }
        }
    }

    /// A skirt cell face-adjacent to the trunk column is never ragged-trimmed:
    /// one missing block right against the log reads as a defect, not as a
    /// ragged silhouette (playtest 2026-08-20). The narrow (radius-1) skirt
    /// layers are where this bites — their whole outer ring hugs the trunk.
    #[test]
    fn conifer_skirts_never_trim_the_trunk_hugging_ring() {
        const FACES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        for radius in [2i32, 3] {
            for seed in [1u32, 7, 42, 1000, 31337] {
                let mut chunk = Chunk::new(0, 0);
                let (cx, cz, base, h) = (8i32, 8i32, 64i32, 9i32);
                let plan = column_trunk(&mut chunk, cx, cz, base, h);
                let top = plan.attach[0];
                let mut rng = FeatureRng::positional(seed, 0xABCD, cx, 0, cz);
                let mut sink = crate::feature::ChunkSink::new(&mut chunk);
                let mut ctx = FeatureCtx::new(&mut sink);
                let cone = ConiferFoliage {
                    radius,
                    skirt_ragged: 0.25,
                };
                cone.place(
                    &mut ctx,
                    &mut |_| true,
                    &plan,
                    Block::SpruceLeaves,
                    &mut rng,
                );

                let max_r = radius.max(2);
                for i in 4..(4 + max_r * 2) {
                    let y = top.y - i;
                    for (dx, dz) in FACES {
                        assert_eq!(
                            chunk.block_raw((cx + dx) as usize, y as usize, (cz + dz) as usize),
                            Block::SpruceLeaves.id(),
                            "r{radius} seed {seed}: skirt {i} hole against the trunk at {dx},{dz}"
                        );
                    }
                }
            }
        }
    }

    /// Against a closed half-space (a hillside / cave wall beside the trunk),
    /// the conifer canopy must place nothing in the closed cells AND leave no
    /// placed leaf without a face-step path to a trunk log through placed
    /// leaves — the exact support the decay flood checks. This is the bug where
    /// terrain interruptions stranded skirt leaves that then rotted, and where
    /// leaves skipped a cave wall and reappeared inside the cave.
    #[test]
    fn conifer_canopy_respects_closed_cells_and_stays_connected() {
        use std::collections::{HashSet, VecDeque};
        for seed in [1u32, 7, 42, 1000, 31337] {
            let mut chunk = Chunk::new(0, 0);
            let (cx, cz, base, h) = (8i32, 8i32, 64i32, 9i32);
            let plan = column_trunk(&mut chunk, cx, cz, base, h);
            let logs: HashSet<(i32, i32, i32)> =
                plan.logs.iter().map(|p| (p.x, p.y, p.z)).collect();
            // Wall right beside the trunk: everything at x > cx is closed.
            let mut open = |p: IVec3| p.x <= cx;
            let mut rng = FeatureRng::positional(seed, 0xABCD, cx, 0, cz);
            let mut sink = crate::feature::ChunkSink::new(&mut chunk);
            let mut ctx = FeatureCtx::new(&mut sink);
            let cone = ConiferFoliage {
                radius: 2,
                skirt_ragged: 0.25,
            };
            cone.place(&mut ctx, &mut open, &plan, Block::SpruceLeaves, &mut rng);

            let mut leaves = HashSet::new();
            for y in 0..200i32 {
                for x in 0..16i32 {
                    for z in 0..16i32 {
                        if chunk.block_raw(x as usize, y as usize, z as usize)
                            == Block::SpruceLeaves.id()
                        {
                            leaves.insert((x, y, z));
                        }
                    }
                }
            }
            assert!(
                !leaves.is_empty(),
                "seed {seed}: the open side must still get a canopy"
            );
            assert!(
                leaves.iter().all(|&(x, _, _)| x <= cx),
                "seed {seed}: a leaf was placed in a closed cell"
            );
            // Every leaf reaches a log via face steps through placed leaves.
            let mut reached: HashSet<(i32, i32, i32)> = HashSet::new();
            let mut frontier: VecDeque<(i32, i32, i32)> = logs.iter().copied().collect();
            while let Some((x, y, z)) = frontier.pop_front() {
                for (dx, dy, dz) in [
                    (1, 0, 0),
                    (-1, 0, 0),
                    (0, 1, 0),
                    (0, -1, 0),
                    (0, 0, 1),
                    (0, 0, -1),
                ] {
                    let n = (x + dx, y + dy, z + dz);
                    if leaves.contains(&n) && reached.insert(n) {
                        frontier.push_back(n);
                    }
                }
            }
            for l in &leaves {
                assert!(
                    reached.contains(l),
                    "seed {seed}: leaf at {l:?} has no face-step path to the trunk — it would decay"
                );
            }
        }
    }
}
