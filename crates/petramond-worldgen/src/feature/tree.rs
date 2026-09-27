use petramond_math::detmath;
use petramond_world::block::Block;
use petramond_world::mathh::IVec3;

use super::placers::foliage::FoliagePlacer;
use super::placers::shapes;
use super::placers::trunk::{sample_height, TrunkPlacer};
use super::{Feature, FeatureCtx};
use crate::rng::FeatureRng;

mod oak;
mod posture;

pub use oak::{BlockyOakFeature, TIP_FENCE};
use posture::{connected_branch, connected_trunk, TrunkPosture};

pub struct TreeFeature {
    pub trunk: &'static dyn TrunkPlacer,
    pub foliage: &'static dyn FoliagePlacer,
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
}

impl Feature for TreeFeature {
    fn is_anchored(
        &self,
        surf: &mut dyn FnMut(i32, i32) -> i32,
        origin: IVec3,
        _: FeatureRng,
    ) -> bool {
        self.trunk.is_anchored(surf, origin)
    }

    fn generate(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        origin: IVec3,
        rng: &mut FeatureRng,
    ) {
        let plan = self.trunk.place(ctx, origin, self.height, self.log, rng);
        self.foliage.place(ctx, open, &plan, self.leaf, rng);
    }
}

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

const CARDINALS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

fn rotate(d: (i32, i32), clockwise: bool) -> (i32, i32) {
    if clockwise {
        (d.1, -d.0)
    } else {
        (-d.1, d.0)
    }
}

fn shuffle<T>(items: &mut [T], rng: &mut FeatureRng) {
    for i in (1..items.len()).rev() {
        let j = rng.next_i32(0, i as i32) as usize;
        items.swap(i, j);
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanopyTreeFeature {
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
    pub split: f32,
    pub lean: (i32, i32),
    pub tip_height: (i32, i32),
    pub limbs: (i32, i32),
    pub reach: (i32, i32),
    pub tip_radius: (i32, i32),
    pub crown_radius: i32,
    pub round: f32,
}

const GOLDEN_ANGLE: f32 = 2.399_963_1;

const CANOPY_MIN_HEIGHT: i32 = 5;
const CANOPY_MAX_LIMBS: i32 = 16;
const CLUMP_RADIUS: (i32, i32) = (2, 3);
const CANOPY_MIN_STEM_CLEARANCE: i64 = 3;

impl CanopyTreeFeature {
    pub fn validate(&self) -> Result<(), String> {
        use crate::data::bounds::{ascending, unit, within};
        ascending("height", self.height, CANOPY_MIN_HEIGHT..=i32::MAX)?;
        ascending("lean", self.lean, 0..=crate::feature::MARGIN)?;
        ascending("reach", self.reach, 0..=crate::feature::MARGIN)?;
        ascending("tip_height", self.tip_height, i32::MIN..=i32::MAX)?;
        ascending("limbs", self.limbs, 1..=CANOPY_MAX_LIMBS)?;
        ascending(
            "tip_radius",
            self.tip_radius,
            CLUMP_RADIUS.0..=CLUMP_RADIUS.1,
        )?;
        within(
            "crown_radius",
            self.crown_radius,
            CLUMP_RADIUS.0..=CLUMP_RADIUS.1,
        )?;
        unit("split", self.split)?;
        if self.split >= 1.0 {
            return Err("split: must stay below 1 so at least one limb forks off".into());
        }
        unit("round", self.round)?;

        let horizontal = i64::from(self.lean.1.max(1))
            + (i64::from(self.reach.1) + i64::from(self.tip_radius.1))
                .max(i64::from(self.crown_radius));
        if horizontal > i64::from(crate::feature::MARGIN) {
            return Err(format!(
                "lean + reach + tip_radius: {horizontal} blocks of crown reach exceed the \
                 {} block replay margin",
                crate::feature::MARGIN
            ));
        }
        let lowest_clump_floor =
            i64::from(self.height.0) + i64::from(self.tip_height.0) - i64::from(self.tip_radius.1);
        if lowest_clump_floor < CANOPY_MIN_STEM_CLEARANCE {
            return Err(format!(
                "height + tip_height - tip_radius: the lowest clump reaches down to {lowest_clump_floor}, \
                 leaving under {CANOPY_MIN_STEM_CLEARANCE} logs of clear stem"
            ));
        }
        let top = i64::from(self.height.1)
            + i64::from(self.tip_height.1.max(0))
            + i64::from(self.tip_radius.1.max(self.crown_radius));
        if top > i64::from(super::MAX_TREE_REACH_ABOVE) {
            return Err(format!(
                "height + tip_height + clump radius: {top} blocks above the anchor exceed the \
                 {} block tree reach",
                super::MAX_TREE_REACH_ABOVE
            ));
        }
        Ok(())
    }
}

impl Feature for CanopyTreeFeature {
    fn generate(
        &self,
        ctx: &mut FeatureCtx,
        _open: &mut dyn FnMut(IVec3) -> bool,
        origin: IVec3,
        rng: &mut FeatureRng,
    ) {
        use std::f32::consts::TAU;
        let (x, y, z) = (origin.x, origin.y, origin.z);
        let height = sample_height(self.height, rng);
        let top = y + height - 1;
        let split_y = y + ((height as f32) * self.split).floor() as i32;

        let posture = TrunkPosture::sample(self.lean, rng);
        let trunk_at = |level| {
            let (dx, dz) = posture.offset(level, height);
            IVec3::new(x + dx, y + level, z + dz)
        };
        let mut previous = origin;
        for level in 0..height {
            let next = trunk_at(level);
            connected_trunk(ctx, previous, next, self.log);
            previous = next;
        }
        let crown = trunk_at(height - 1);

        let limbs = sample_height(self.limbs, rng);
        let base_angle = rng.next_f32() * TAU;
        let span = (top - 1 - split_y).max(0);
        for i in 0..limbs {
            let t = if limbs > 1 {
                i as f32 / (limbs - 1) as f32
            } else {
                0.5
            };
            let node_y = (split_y + (span as f32 * t).round() as i32 + rng.next_i32(-1, 1))
                .clamp(split_y, top - 1);
            let angle = base_angle + i as f32 * GOLDEN_ANGLE;
            let reach = sample_height(self.reach, rng);
            let tip = IVec3::new(
                crown.x + (detmath::cosf(angle) * reach as f32).round() as i32,
                top + sample_height(self.tip_height, rng),
                crown.z + (detmath::sinf(angle) * reach as f32).round() as i32,
            );
            let tip_r = sample_height(self.tip_radius, rng);
            let node = trunk_at(node_y.min(tip.y) - y);
            let elbow = IVec3::new(
                node.x + (tip.x - node.x) / 2,
                node.y + (tip.y - node.y) / 3,
                node.z + (tip.z - node.z) / 2,
            );
            connected_branch(ctx, node, elbow, self.log);
            connected_branch(ctx, elbow, tip, self.log);
            shapes::leaf_blob_rounded(ctx, tip, tip_r, self.leaf, self.round, rng);
        }

        shapes::leaf_blob_rounded(ctx, crown, self.crown_radius, self.leaf, self.round, rng);
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedwoodFeature {
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
}

pub const REDWOOD_BASE_SUPPORT_REACH: i32 = 5;

const REDWOOD_CROWN_OVERHANG: i32 = 4;
const REDWOOD_MIN_HEIGHT: i32 = 12;

impl RedwoodFeature {
    pub fn validate(&self) -> Result<(), String> {
        crate::data::bounds::ascending(
            "height",
            self.height,
            REDWOOD_MIN_HEIGHT..=super::MAX_TREE_REACH_ABOVE - REDWOOD_CROWN_OVERHANG,
        )
    }
}

const REDWOOD_CANOPY_ROUND: f32 = 0.7;

fn disc_contains(dx: i32, dz: i32, radius: f32) -> bool {
    let fx = dx.abs() as f32 + 0.5;
    let fz = dz.abs() as f32 + 0.5;
    fx * fx + fz * fz <= radius * radius
}

pub fn redwood_base_trunk_contains(dx: i32, dz: i32) -> bool {
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
    let stem = 0.80 + 2.35 * detmath::powf(1.0 - t, 0.85);
    let flare = if t < 0.22 {
        1.45 * detmath::powf(1.0 - t / 0.22, 1.7)
    } else {
        0.0
    };
    stem + flare
}

impl Feature for RedwoodFeature {
    fn generate(
        &self,
        ctx: &mut FeatureCtx,
        _open: &mut dyn FnMut(IVec3) -> bool,
        origin: IVec3,
        rng: &mut FeatureRng,
    ) {
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
                x + (detmath::cosf(angle) * reach as f32).round() as i32,
                node_y + rng.next_i32(-1, 1) + if t > 0.72 { 1 } else { 0 },
                z + (detmath::sinf(angle) * reach as f32).round() as i32,
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
