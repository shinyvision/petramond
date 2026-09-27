use std::collections::VecDeque;

use crate::feature::placers::trunk::TrunkPlan;
use crate::feature::FeatureCtx;
use crate::rng::FeatureRng;
use petramond_world::block::behavior::MAX_LOG_DISTANCE;
use petramond_world::block::Block;
use petramond_world::mathh::{IVec3, FACE_NEIGHBORS};

mod layered;
pub use layered::LayeredFoliage;

pub trait FoliagePlacer: Send + Sync {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        trunk: &TrunkPlan,
        leaf: Block,
        rng: &mut FeatureRng,
    );

    fn horizontal_reach(&self) -> i32;
}

/// Candidate canopy cells, recorded in draw order and committed after a connectivity check.
///
/// Split on purpose: RNG draw order must stay exactly what the loops consumed, but which cells get
/// written only depends on (draws, `open`), both world-anchored. So every chunk replaying the tree
/// keeps the same cells. A cell only lands if a face-step path of open candidates reaches a trunk
/// log within [`MAX_LOG_DISTANCE`] steps, matching the leaf-decay flood. Anything `open` rejects
/// gets dropped, along with anything that only connected through it. That way nothing leaks
/// through solid ground into a cave and no leaf decays right after generation.
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

        for &p in &self.cells {
            let i = idx(p);
            if grid[i] & CANDIDATE == 0 {
                grid[i] |= CANDIDATE;
                if open(p) {
                    grid[i] |= OPEN;
                }
            }
        }

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
                continue;
            }
            let outer = lx.abs() == radius || lz.abs() == radius;
            let hugs_centre = lx.abs() + lz.abs() == 1;
            if outer && ragged > 0.0 && rng.chance(ragged) && !hugs_centre {
                continue;
            }
            canopy.add(IVec3::new(cx + lx, y, cz + lz));
        }
    }
}

fn plus_ring(canopy: &mut Canopy, cx: i32, y: i32, cz: i32) {
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        canopy.add(IVec3::new(cx + dx, y, cz + dz));
    }
}

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
        leaf_layer(&mut canopy, cx, ct, cz, r, self.ragged, rng);
        leaf_layer(&mut canopy, cx, ct + 1, cz, r - 1, 0.0, rng);
        for lx in -r..=r {
            for lz in -r..=r {
                if lx.abs() == r && lz.abs() == r {
                    continue;
                }
                if !(lx.abs() == r || lz.abs() == r) {
                    continue;
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

/// Spruce/pine: pointed top (leaf tip, '+' at the top log, '+' three down), then ragged skirts
/// that widen for the droopy look. `radius` is skirt size, min 2 or the top gets eaten.
/// `skirt_ragged` is the outer-ring trim chance per skirt.
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

        canopy.add(IVec3::new(a.x, a.y + 1, a.z));
        plus_ring(&mut canopy, a.x, a.y, a.z);
        plus_ring(&mut canopy, a.x, a.y - 2, a.z);

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
        let ur = self.upper_radius;
        for lx in -ur..=ur {
            for lz in -ur..=ur {
                let d = lx.abs() + lz.abs();
                if d > ur {
                    continue;
                }
                if d == ur && rng.chance(self.upper_skip) {
                    continue;
                }
                canopy.add(IVec3::new(cx + lx, ct + 1, cz + lz));
            }
        }
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

#[cfg(test)]
mod spruce_tests {
    use super::*;
    use crate::rng::FeatureRng;
    use petramond_world::chunk::Chunk;

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

    /// Conifer canopy against a closed half-space (hillside or cave wall next to the trunk):
    /// nothing goes in closed cells, and every placed leaf has a face-step path back to a trunk
    /// log through other leaves, matching the decay flood checks. This keeps
    /// leaves out of closed terrain and connected to the trunk.
    #[test]
    fn conifer_canopy_respects_closed_cells_and_stays_connected() {
        use std::collections::{HashSet, VecDeque};
        for seed in [1u32, 7, 42, 1000, 31337] {
            let mut chunk = Chunk::new(0, 0);
            let (cx, cz, base, h) = (8i32, 8i32, 64i32, 9i32);
            let plan = column_trunk(&mut chunk, cx, cz, base, h);
            let logs: HashSet<(i32, i32, i32)> =
                plan.logs.iter().map(|p| (p.x, p.y, p.z)).collect();
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
