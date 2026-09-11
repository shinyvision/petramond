use super::*;
use crate::feature::placers::shapes::connected_line;

/// A tapered stem with rising side limbs distributed through a vertical band.
/// Each limb tip and the stem top is an independent foliage attachment.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhorledTrunk {
    pub limbs: (i32, i32),
    pub band: (f32, f32),
    pub reach: (i32, i32),
    pub rise: (i32, i32),
    pub flare: i32,
    pub stem_fraction: f32,
    pub elbow_rise: f32,
    pub limb_radius: i32,
}

impl WhorledTrunk {
    fn ground_cells(&self, mut visit: impl FnMut(i32, i32)) {
        let radius = i32::from(self.flare > 0);
        for x in -radius..=radius {
            for z in -radius..=radius {
                visit(x, z);
            }
        }
        for distance in 2..=self.flare {
            for [dx, dz] in [[1, 0], [-1, 0], [0, 1], [0, -1]] {
                visit(dx * distance, dz * distance);
            }
        }
    }

    pub fn validate(&self, height: (i32, i32)) -> Result<(), String> {
        use crate::data::bounds::{ascending, unit, unit_range, within};
        ascending("limbs", self.limbs, 1..=16)?;
        ascending("reach", self.reach, 1..=8)?;
        ascending("rise", self.rise, 0..=12)?;
        unit_range("band", self.band)?;
        unit("stem_fraction", self.stem_fraction)?;
        unit("elbow_rise", self.elbow_rise)?;
        if self.stem_fraction < 0.2 || self.band.1 > self.stem_fraction {
            return Err(
                "branch band must fit the stem, which must occupy at least 0.2 of the height"
                    .into(),
            );
        }
        within("limb_radius", self.limb_radius, 0..=2)?;
        let lowest_limb = (self.band.0 * (height.0 - 1) as f32).round() as i32;
        if lowest_limb <= self.limb_radius {
            return Err("lowest limb must clear the ground with its full radius".into());
        }
        within("flare", self.flare, 0..=3)
    }
}

impl TrunkPlacer for WhorledTrunk {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> TrunkPlan {
        let height = sample_height(height, rng);
        let stem_top = ((height - 1) as f32 * self.stem_fraction).round() as i32;
        let mut plan = TrunkPlan {
            attach: if self.stem_fraction == 1.0 {
                vec![origin + IVec3::Y * stem_top]
            } else {
                Vec::new()
            },
            logs: Vec::new(),
        };
        let mut emit = |p| {
            ctx.set_log(p, log);
            plan.logs.push(p);
        };
        for y in 0..=stem_top {
            let radius = i32::from(y < height / 2 && self.flare > 0);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    emit(origin + IVec3::new(x, y, z));
                }
            }
        }
        self.ground_cells(|x, z| {
            let distance = x.abs().max(z.abs());
            let top = ((self.flare - distance) * 2).max(0).min(stem_top);
            for y in 0..=top {
                emit(origin + IVec3::new(x, y, z));
            }
        });
        let limbs = sample_height(self.limbs, rng);
        let angle = rng.next_f32() * std::f32::consts::TAU;
        for limb in 0..limbs {
            let t = limb as f32 / (limbs - 1).max(1) as f32;
            let y = ((self.band.0 + t * (self.band.1 - self.band.0)) * (height - 1) as f32).round()
                as i32;
            let direction = angle + limb as f32 * 2.399_963_1;
            let reach = sample_height(self.reach, rng) as f32;
            let tip = origin
                + IVec3::new(
                    (direction.cos() * reach).round() as i32,
                    (y + sample_height(self.rise, rng)).min(height - 1),
                    (direction.sin() * reach).round() as i32,
                );
            let root = origin + IVec3::Y * y;
            let elbow = root
                + IVec3::new(
                    (tip.x - root.x) / 2,
                    ((tip.y - root.y) as f32 * self.elbow_rise).round() as i32,
                    (tip.z - root.z) / 2,
                );
            let length = (tip - root).as_vec3().length().max(1.0);
            let mut limb = |p: IVec3| {
                let t = (p - root).as_vec3().length() / length;
                let radius = (self.limb_radius as f32 * (1.0 - t)).round() as i32;
                for x in -radius..=radius {
                    for y in -radius..=radius {
                        for z in -radius..=radius {
                            if x * x + y * y + z * z <= radius * radius && p.y + y >= origin.y {
                                emit(p + IVec3::new(x, y, z));
                            }
                        }
                    }
                }
            };
            connected_line(root, elbow, &mut limb);
            connected_line(elbow, tip, &mut limb);
            plan.attach.push(tip);
        }
        plan
    }

    fn max_lean(&self) -> i32 {
        (self.reach.1 + self.limb_radius).max(self.flare)
    }

    fn is_anchored(&self, surf: &mut dyn FnMut(i32, i32) -> i32, origin: IVec3) -> bool {
        let mut anchored = true;
        self.ground_cells(|x, z| {
            anchored &= surf(origin.x + x, origin.z + z) >= origin.y - 1;
        });
        anchored
    }
}

#[cfg(test)]
mod tests;
