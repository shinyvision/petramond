//! Tree features.
//!
//! `TreeFeature` is the generic composition — one `TrunkPlacer` + one
//! `FoliagePlacer` + materials + params — that expresses the four "normal" oaks
//! (and any future birch/pine/etc.) as pure data. `GiantOakFeature` is the
//! large 2×2 branching oak: its branches and crown are spatially entangled
//! (a branch's logs and its tip leaves interleave with the next branch), so it
//! is its own `Feature` rather than a clean trunk/foliage split — mirroring how
//! real engines model complex trees. It still reuses the shared `shapes`
//! primitives and the same `FeatureCtx` predicates.

use crate::block::Block;
use crate::mathh::IVec3;

use super::placers::foliage::FoliagePlacer;
use super::placers::shapes;
use super::placers::trunk::{sample_height, TrunkPlacer};
use super::{Feature, FeatureCtx};
use crate::worldgen::rng::FeatureRng;

/// Generic single-trunk tree: trunk places + returns attach points, then
/// foliage decorates them.
pub struct TreeFeature {
    pub trunk: &'static dyn TrunkPlacer,
    pub foliage: &'static dyn FoliagePlacer,
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
    pub radius: i32,
    pub footprint: i32,
}

impl Feature for TreeFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        let attach = self.trunk.place(ctx, origin, self.height, self.log, rng);
        self.foliage.place(ctx, &attach, self.radius, self.leaf, rng);
    }
    fn max_footprint(&self) -> i32 {
        self.footprint
    }
}

/// Big 2×2 branching oak (== `trees::oak_big`).
pub struct GiantOakFeature {
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
    pub footprint: i32,
}

impl Feature for GiantOakFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        let (x, y, z) = (origin.x, origin.y, origin.z);
        // Reserve a 2×2 footprint; caller already skipped chunk edges.
        if x + 1 >= 16 || z + 1 >= 16 {
            return;
        }
        let height = sample_height(self.height, rng); // == 8 + next_i32(0,4)
        let base = y;
        // Trunk: 2×2 column.
        for i in 0..height {
            let h = base + i;
            ctx.set_log(IVec3::new(x, h, z), self.log);
            ctx.set_log(IVec3::new(x + 1, h, z), self.log);
            ctx.set_log(IVec3::new(x, h, z + 1), self.log);
            ctx.set_log(IVec3::new(x + 1, h, z + 1), self.log);
        }
        // Branches: from ~70% height, walk diagonally out/up; leaf blob at tip.
        let crown_base = base + (height * 7 / 10);
        let branch_count = rng.next_i32(2, 4);
        for _ in 0..branch_count {
            let sx = x + rng.next_i32(0, 1);
            let sz = z + rng.next_i32(0, 1);
            let sy = crown_base + rng.next_i32(0, 2);
            let (bdx, bdz) = match rng.next_i32(0, 7) {
                0 => (-1, 0),
                1 => (1, 0),
                2 => (0, -1),
                3 => (0, 1),
                4 => (-1, -1),
                5 => (-1, 1),
                6 => (1, -1),
                _ => (1, 1),
            };
            let len = rng.next_i32(2, 4);
            let (mut bx, mut by, mut bz) = (sx, sy, sz);
            for _ in 0..len {
                bx += bdx;
                by += 1;
                bz += bdz;
                ctx.set_branch(IVec3::new(bx, by, bz), self.log);
            }
            shapes::leaf_blob(ctx, IVec3::new(bx, by, bz), 2, self.leaf, false);
        }
        // Crown: layered leaves around the 2×2 top center.
        let top = base + height - 1;
        let cx = x + 1;
        let cz = z + 1;
        // Layer 0 (just below top): radius 2.
        for lx in -2i32..=2 {
            for lz in -2i32..=2 {
                if lx.abs() == 2 && lz.abs() == 2 {
                    continue;
                }
                ctx.set_leaf(IVec3::new(cx + lx, top - 1, cz + lz), self.leaf);
            }
        }
        // Layer 1 (top): radius 1, plus corners randomly.
        for lx in -1i32..=1 {
            for lz in -1i32..=1 {
                if lx == 0 && lz == 0 {
                    ctx.set_leaf(IVec3::new(cx, top + 1, cz), self.leaf);
                    continue;
                }
                if (lx.abs() == 1 && lz.abs() == 1) && rng.chance(0.5) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(cx + lx, top, cz + lz), self.leaf);
            }
        }
        // Layer 2 (above): small cap.
        for lx in -1i32..=1 {
            for lz in -1i32..=1 {
                if lx.abs() == 1 && lz.abs() == 1 {
                    continue;
                }
                if rng.chance(0.4) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(cx + lx, top + 1, cz + lz), self.leaf);
            }
        }
    }

    fn max_footprint(&self) -> i32 {
        self.footprint
    }
}
