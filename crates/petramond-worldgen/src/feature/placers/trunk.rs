use crate::feature::FeatureCtx;
use crate::rng::FeatureRng;
use petramond_world::block::Block;
use petramond_world::mathh::IVec3;

mod whorled;
pub use whorled::WhorledTrunk;

pub struct TrunkPlan {
    pub attach: Vec<IVec3>,
    pub logs: Vec<IVec3>,
}

pub trait TrunkPlacer: Send + Sync {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> TrunkPlan;

    fn max_lean(&self) -> i32 {
        0
    }

    fn is_anchored(&self, _: &mut dyn FnMut(i32, i32) -> i32, _: IVec3) -> bool {
        true
    }
}

#[inline]
pub fn sample_height(height: (i32, i32), rng: &mut FeatureRng) -> i32 {
    if height.0 < height.1 {
        rng.next_i32(height.0, height.1)
    } else {
        height.0
    }
}

pub struct StraightTrunk;

impl TrunkPlacer for StraightTrunk {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> TrunkPlan {
        let h = sample_height(height, rng);
        let mut logs = Vec::with_capacity(h as usize);
        for i in 0..h {
            let p = IVec3::new(origin.x, origin.y + i, origin.z);
            ctx.set_log(p, log);
            logs.push(p);
        }
        TrunkPlan {
            attach: vec![IVec3::new(origin.x, origin.y + h - 1, origin.z)],
            logs,
        }
    }
}

pub struct LeaningTrunk;

impl TrunkPlacer for LeaningTrunk {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> TrunkPlan {
        let h = sample_height(height, rng);
        let dx = rng.next_i32(-1, 1);
        let dz = rng.next_i32(-1, 1);
        let (mut cx, mut cz) = (origin.x, origin.z);
        let mut logs = Vec::with_capacity(h as usize);
        for i in 0..h {
            let p = IVec3::new(cx, origin.y + i, cz);
            ctx.set_log(p, log);
            logs.push(p);
            if i == h / 2 {
                cx += dx;
                cz += dz;
            }
        }
        TrunkPlan {
            attach: vec![IVec3::new(cx, origin.y + h - 1, cz)],
            logs,
        }
    }

    fn max_lean(&self) -> i32 {
        1
    }
}
