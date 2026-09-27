use crate::rng::patch_field;
use petramond_world::block::Block;
use petramond_world::chunk::SEA_LEVEL;

pub enum SurfaceRule {
    Block(Block),
    Sequence(&'static [SurfaceRule]),
    Condition {
        when: SurfaceCond,
        then: &'static SurfaceRule,
    },
}

pub enum SurfaceCond {
    SurfaceAboveY(i32),
    DepthFromTop(u32),
    Underwater,
    /// True when a smooth low-freq value-noise draw in `[0,1)` is under `threshold`.
    /// Columns share corner samples, so hits cluster into period-sized patches instead
    /// of per-column speckle, say grass clumps on podzol. `period` sets patch size in blocks.
    ClusterNoiseBelow {
        salt: u64,
        threshold: f32,
        period: f32,
    },
}

pub struct SurfaceCtx {
    pub seed: u32,
    pub wx: i32,
    pub wz: i32,
    pub y: i32,
    pub surf_y: i32,
    pub depth_from_top: u32,
}

impl SurfaceCond {
    #[inline]
    fn test(&self, c: &SurfaceCtx) -> bool {
        match self {
            SurfaceCond::SurfaceAboveY(n) => c.surf_y > *n,
            SurfaceCond::DepthFromTop(n) => c.depth_from_top <= *n,
            SurfaceCond::Underwater => c.surf_y < SEA_LEVEL,
            SurfaceCond::ClusterNoiseBelow {
                salt,
                threshold,
                period,
            } => patch_field(c.seed, *salt, c.wx, c.wz, *period) < *threshold,
        }
    }
}

impl SurfaceRule {
    pub fn deepest_band(&self) -> Option<u32> {
        match self {
            SurfaceRule::Block(_) => None,
            SurfaceRule::Sequence(rules) => {
                rules.iter().filter_map(SurfaceRule::deepest_band).max()
            }
            SurfaceRule::Condition { when, then } => {
                let own = match when {
                    SurfaceCond::DepthFromTop(n) => Some(*n),
                    _ => None,
                };
                own.max(then.deepest_band())
            }
        }
    }

    pub fn resolve(&self, c: &SurfaceCtx) -> Option<Block> {
        match self {
            SurfaceRule::Block(b) => Some(*b),
            SurfaceRule::Sequence(rules) => rules.iter().find_map(|r| r.resolve(c)),
            SurfaceRule::Condition { when, then } => {
                if when.test(c) {
                    then.resolve(c)
                } else {
                    None
                }
            }
        }
    }
}
