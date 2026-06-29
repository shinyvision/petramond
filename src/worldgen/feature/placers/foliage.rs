//! Foliage placers — build a tree's leaves around the trunk's attach points.
//!
//! Each placer is a *family* of canopy shape (broadleaf blob, conifer cone,
//! droopy swamp, flat umbrella). The per-tree differences — width, raggedness,
//! drip chance — are FIELDS on the placer, so a new look is a data row in
//! `data::features` (a new `BlobFoliage { .. }`), not a new impl. A genuinely
//! new *shape* (different layer profile, entangled branches) is a new placer or
//! a bespoke `Feature` instead.
//!
//! Iteration order and the per-cell RNG draws are fixed so cross-chunk seam
//! replay stays deterministic (a tree rooted in a neighbour materialises
//! identically here). Parameterising never reorders or adds draws: each field
//! simply names a constant the loop already consumed.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::worldgen::feature::FeatureCtx;
use crate::worldgen::rng::FeatureRng;

pub trait FoliagePlacer: Send + Sync {
    fn place(&self, ctx: &mut FeatureCtx, attach: &[IVec3], leaf: Block, rng: &mut FeatureRng);
}

/// Place a square leaf layer of the given radius at world Y `y`, centred on
/// `(cx, cz)`. Hard corners (|lx|==|lz|==radius) are always cut; outer-ring cells
/// are trimmed with probability `ragged` for a natural edge. Over Air/Water only.
fn leaf_layer(
    ctx: &mut FeatureCtx,
    cx: i32,
    y: i32,
    cz: i32,
    radius: i32,
    leaf: Block,
    ragged: f32,
    rng: &mut FeatureRng,
) {
    for lx in -radius..=radius {
        for lz in -radius..=radius {
            if lx.abs() == radius && lz.abs() == radius {
                continue; // cut hard corners
            }
            let outer = lx.abs() == radius || lz.abs() == radius;
            if outer && ragged > 0.0 && rng.chance(ragged) {
                continue; // ragged edge
            }
            ctx.set_leaf(IVec3::new(cx + lx, y, cz + lz), leaf);
        }
    }
}

/// Place the four cardinal-neighbour leaves around `(cx, y, cz)` — a
/// deterministic '+' with no ragged trimming (every face is always filled). The
/// centre cell is left to the trunk log, which `set_leaf` won't overwrite.
fn plus_ring(ctx: &mut FeatureCtx, cx: i32, y: i32, cz: i32, leaf: Block) {
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        ctx.set_leaf(IVec3::new(cx + dx, y, cz + dz), leaf);
    }
}

/// Broadleaf-oak-style blob canopy: four square leaf layers attaching one block
/// ABOVE the top log (`topY = topLog + 1`), running `topY-3 ..= topY`. The two
/// BOTTOM layers use `base_radius`, the two TOP layers use `top_radius` (a
/// fuller, rounder blob than a single wide layer). The ONLY cells ever removed
/// are the four extreme corners (`|dx|==r && |dz|==r`): always on the very top
/// layer, and with probability `corner_cut` on the others. (We deliberately do
/// NOT trim the whole outer ring — that over-thins the canopy into a scraggly
/// look.) Shared by oak/birch (small) and jungle/dark-oak/cherry (large); a new
/// width or raggedness is a new data row, not a new impl.
///
/// `base_radius` / `top_radius` must be ≥ 1.
pub struct BlobFoliage {
    pub base_radius: i32,
    pub top_radius: i32,
    pub corner_cut: f32,
}

impl FoliagePlacer for BlobFoliage {
    fn place(&self, ctx: &mut FeatureCtx, attach: &[IVec3], leaf: Block, rng: &mut FeatureRng) {
        let a = attach[0];
        let top_y = a.y + 1; // leaves attach one block above the highest log
                             // (dy from top, layer radius) bottom→top: wide, wide, narrow, narrow.
        let layers = [
            (-3, self.base_radius),
            (-2, self.base_radius),
            (-1, self.top_radius),
            (0, self.top_radius),
        ];
        for (dy, lr) in layers {
            let y = top_y + dy;
            let is_top = dy == 0;
            for lx in -lr..=lr {
                for lz in -lr..=lr {
                    if lx.abs() == lr && lz.abs() == lr {
                        // Only the 4 extreme corners are removable: always on the
                        // top layer, `corner_cut` elsewhere (one draw per corner).
                        if is_top || rng.chance(self.corner_cut) {
                            continue;
                        }
                    }
                    ctx.set_leaf(IVec3::new(a.x + lx, y, a.z + lz), leaf);
                }
            }
        }
    }
}

/// Droopy swamp canopy: a wide flat main layer at the trunk top, a small cap one
/// block above (`radius - 1`), and leaves that hang one block down from the outer
/// ring (the swamp "drip"). `ragged` trims the main layer's edge; `drip_skip` is
/// the chance to omit each individual hanging drip.
pub struct DroopyFoliage {
    pub radius: i32,
    pub ragged: f32,
    pub drip_skip: f32,
}

impl FoliagePlacer for DroopyFoliage {
    fn place(&self, ctx: &mut FeatureCtx, attach: &[IVec3], leaf: Block, rng: &mut FeatureRng) {
        let a = attach[0];
        let (cx, cz, ct) = (a.x, a.z, a.y);
        let r = self.radius;
        // Wide main layer + a small cap.
        leaf_layer(ctx, cx, ct, cz, r, leaf, self.ragged, rng);
        leaf_layer(ctx, cx, ct + 1, cz, r - 1, leaf, 0.0, rng);
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
                ctx.set_leaf(IVec3::new(cx + lx, ct - 1, cz + lz), leaf);
            }
        }
    }
}

/// Conifer canopy (spruce / pine): a deterministic pointed top — a single-leaf
/// tip, a '+'-crown hugging the top log, and a second '+' on the third block
/// down (all four faces always filled) — over widening ragged "skirts" that give
/// the canonical drooping evergreen silhouette. `radius` controls how wide/tall the
/// skirts grow (clamped to ≥2 so the pointed top stays intact); `skirt_ragged`
/// is the outer-ring trim chance per skirt.
pub struct ConiferFoliage {
    pub radius: i32,
    pub skirt_ragged: f32,
}

impl FoliagePlacer for ConiferFoliage {
    fn place(&self, ctx: &mut FeatureCtx, attach: &[IVec3], leaf: Block, rng: &mut FeatureRng) {
        let a = attach[0];
        let max_r = self.radius.max(2);

        // Deterministic pointed top, so a spruce is never bald or lopsided up
        // there: a single-leaf tip, a '+'-crown around the top log, and a second
        // '+' on the third block down. Both rings are ALWAYS four full faces (no
        // ragged trimming); their centres are trunk logs that `set_leaf` keeps.
        ctx.set_leaf(IVec3::new(a.x, a.y + 1, a.z), leaf); // tip
        plus_ring(ctx, a.x, a.y, a.z, leaf); // crown (1st block from top)
        plus_ring(ctx, a.x, a.y - 2, a.z, leaf); // 3rd block from top

        // Widening ragged skirts below the top three blocks: a two-step
        // wide/narrow cycle for the drooping conifer silhouette. The top is
        // placed above, so descent starts at the first skirt. Iteration and
        // per-cell draw order are fixed for deterministic cross-chunk replay.
        let layers = 4 + max_r * 2;
        for i in 4..layers {
            let y = a.y - i;
            let grow = (i / 2).min(max_r);
            let r = if i % 2 == 1 { (grow - 1).max(0) } else { grow };
            leaf_layer(ctx, a.x, y, a.z, r, leaf, self.skirt_ragged, rng);
        }
    }
}

/// Flat sparse savanna canopy (acacia-like silhouette): a thin diamond umbrella
/// spread above a tall trunk, with gaps so it reads as airy. `upper_*` is the
/// raised umbrella disc; `lower_*` is a sparser ring one block below it.
pub struct FlatSparseFoliage {
    pub upper_radius: i32,
    pub upper_skip: f32,
    pub lower_radius: i32,
    pub lower_skip: f32,
}

impl FoliagePlacer for FlatSparseFoliage {
    fn place(&self, ctx: &mut FeatureCtx, attach: &[IVec3], leaf: Block, rng: &mut FeatureRng) {
        let a = attach[0];
        let (cx, cz, ct) = (a.x, a.z, a.y);
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
                ctx.set_leaf(IVec3::new(cx + lx, ct + 1, cz + lz), leaf);
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
                ctx.set_leaf(IVec3::new(cx + lx, ct, cz + lz), leaf);
            }
        }
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod spruce_tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::worldgen::rng::FeatureRng;

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
                for i in 0..h {
                    let p = ((base + i) as usize, cx as usize, cz as usize);
                    chunk.set_block_raw(p.1, p.0, p.2, Block::SpruceLog.id());
                }
                let top = IVec3::new(cx, base + h - 1, cz);
                let mut rng = FeatureRng::positional(seed, 0xABCD, cx, 0, cz);
                let mut sink = crate::worldgen::feature::ChunkSink::new(&mut chunk);
                let mut ctx = FeatureCtx::new(&mut sink);
                let cone = ConiferFoliage {
                    radius,
                    skirt_ragged: 0.25,
                };
                cone.place(&mut ctx, &[top], Block::SpruceLeaves, &mut rng);

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
}
