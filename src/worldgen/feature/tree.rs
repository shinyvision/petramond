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
}

impl Feature for TreeFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        let attach = self.trunk.place(ctx, origin, self.height, self.log, rng);
        self.foliage.place(ctx, &attach, self.leaf, rng);
    }
}

/// Big "fancy oak": a SINGLE-column trunk (the 2×2
/// trunk belongs to dark oak / jungle, not oak), a handful of limbs angling up
/// and out to leaf blobs, and a rounded central crown of stacked leaf discs.
pub struct GiantOakFeature {
    pub log: Block,
    pub leaf: Block,
    /// {min,max} of the nominal total height H; bare trunk = floor(H·0.618).
    pub height: (i32, i32),
}

/// Draw a straight 3-D line of branch logs from `a` to `b` (== fancy-oak limbs).
/// Uses `set_branch` so a limb may pass through already-placed leaves.
fn log_line(ctx: &mut FeatureCtx, a: IVec3, b: IVec3, log: Block) {
    let n = (b.x - a.x)
        .abs()
        .max((b.y - a.y).abs())
        .max((b.z - a.z).abs())
        .max(1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = (a.x as f32 + (b.x - a.x) as f32 * t).round() as i32;
        let y = (a.y as f32 + (b.y - a.y) as f32 * t).round() as i32;
        let z = (a.z as f32 + (b.z - a.z) as f32 * t).round() as i32;
        ctx.set_branch(IVec3::new(x, y, z), log);
    }
}

impl Feature for GiantOakFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        use std::f32::consts::TAU;
        let (x, y, z) = (origin.x, origin.y, origin.z);
        let height = sample_height(self.height, rng); // nominal total height H
        let trunk_h = (height as f32 * 0.618).floor() as i32; // bare lower trunk
        let trunk_top = y + trunk_h;
        let spine_top = y + height - 1; // central log spine runs to near the top

        // Single 1×1 trunk + spine.
        for h in y..=spine_top {
            ctx.set_log(IVec3::new(x, h, z), self.log);
        }

        // Limbs: a few branches up and out, each capped by a spherical leaf blob.
        // Draw order is fixed so a neighbour chunk replays the tree identically.
        let branches = rng.next_i32(3, 5);
        let span = (spine_top - 1 - trunk_top).max(0);
        for _ in 0..branches {
            let ang = rng.next_f32() * TAU;
            let reach = 2 + rng.next_i32(0, 1); // 2..3 blocks out
            let node_y = trunk_top + rng.next_i32(0, span);
            let tip = IVec3::new(
                x + (ang.cos() * reach as f32).round() as i32,
                node_y + rng.next_i32(0, 1),
                z + (ang.sin() * reach as f32).round() as i32,
            );
            let base = IVec3::new(x, (node_y - 1).max(trunk_top), z);
            log_line(ctx, base, tip, self.log);
            shapes::leaf_blob(ctx, tip, 2, self.leaf, false);
        }

        // Rounded central crown over the trunk top: stacked leaf discs with the
        // canonical 2,3,3,2,1 radius profile (== `crossSection` per layer).
        let crown_base = (spine_top - 3).max(trunk_top);
        for (k, r) in [(0, 2.0f32), (1, 3.0), (2, 3.0), (3, 2.0), (4, 1.0)] {
            shapes::leaf_disc(ctx, IVec3::new(x, crown_base + k, z), r, self.leaf);
        }
    }
}

/// Huge redwood: a flared multi-block trunk that tapers upward, long upper limbs
/// with leaf masses, and a narrow high crown. Materials are placeholders (oak
/// log/leaf) until dedicated redwood assets exist.
pub struct RedwoodFeature {
    pub log: Block,
    pub leaf: Block,
    /// {min,max} of the nominal total height H.
    pub height: (i32, i32),
}

pub(crate) const REDWOOD_BASE_SUPPORT_REACH: i32 = 5;

/// Per-corner trim chance for the redwood whorl leaf masses — high enough that the
/// small (r=2) clumps read as rounded blobs rather than the solid cubes a plain
/// `leaf_blob` produces at that radius.
const REDWOOD_CANOPY_ROUND: f32 = 0.7;

fn disc_contains(dx: i32, dz: i32, radius: f32) -> bool {
    let fx = dx.abs() as f32 + 0.5;
    let fz = dz.abs() as f32 + 0.5;
    fx * fx + fz * fz <= radius * radius
}

pub(crate) fn redwood_base_trunk_contains(dx: i32, dz: i32) -> bool {
    disc_contains(dx, dz, redwood_trunk_radius(0, 1))
}

fn log_disc(ctx: &mut FeatureCtx, center: IVec3, radius: f32, log: Block) {
    let ri = radius.ceil() as i32;
    for dx in -ri..=ri {
        for dz in -ri..=ri {
            if disc_contains(dx, dz, radius) {
                ctx.set_log(IVec3::new(center.x + dx, center.y, center.z + dz), log);
            }
        }
    }
}

fn redwood_trunk_radius(level: i32, height: i32) -> f32 {
    let t = level as f32 / (height - 1).max(1) as f32;
    let stem = 0.80 + 2.35 * (1.0 - t).powf(0.85);
    let flare = if t < 0.22 {
        1.45 * (1.0 - t / 0.22).powf(1.7)
    } else {
        0.0
    };
    stem + flare
}

impl Feature for RedwoodFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        use std::f32::consts::TAU;
        let (x, y, z) = (origin.x, origin.y, origin.z);
        let height = sample_height(self.height, rng);
        let spine_top = y + height - 1;

        for level in 0..height {
            log_disc(
                ctx,
                IVec3::new(x, y + level, z),
                redwood_trunk_radius(level, height),
                self.log,
            );
        }

        let crown_base = y + (height as f32 * 0.38).floor() as i32;
        let crown_span = (spine_top - crown_base - 2).max(1);
        let whorls = (height / 4).clamp(9, 14);
        for i in 0..whorls {
            let t = if whorls > 1 {
                i as f32 / (whorls - 1) as f32
            } else {
                0.0
            };
            let node_y =
                (crown_base + (crown_span as f32 * t).round() as i32 + rng.next_i32(-1, 1))
                    .clamp(crown_base, spine_top - 1);
            let angle = rng.next_f32() * TAU + i as f32 * 2.399_963_1;
            let reach_base = 7.0 - 4.5 * t;
            let reach = (reach_base.round() as i32 + rng.next_i32(-1, 1)).clamp(2, 7);
            let tip = IVec3::new(
                x + (angle.cos() * reach as f32).round() as i32,
                node_y + rng.next_i32(-1, 1) + if t > 0.72 { 1 } else { 0 },
                z + (angle.sin() * reach as f32).round() as i32,
            );
            log_line(ctx, IVec3::new(x, node_y, z), tip, self.log);
            shapes::leaf_blob_rounded(
                ctx,
                tip,
                if t > 0.78 { 3 } else { 2 },
                self.leaf,
                REDWOOD_CANOPY_ROUND,
                rng,
            );
        }

        let top_crown_base = (spine_top - 10).max(crown_base);
        for (k, r) in [
            (0, 4.0f32),
            (1, 4.0),
            (2, 3.5),
            (3, 3.0),
            (4, 3.0),
            (5, 2.5),
            (6, 2.0),
            (7, 2.0),
            (8, 1.5),
            (9, 1.0),
            (10, 0.75),
        ] {
            shapes::leaf_disc(ctx, IVec3::new(x, top_crown_base + k, z), r, self.leaf);
        }
    }
}
