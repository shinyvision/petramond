//! Which fluids a body's boxes actually touch, and the caller-owned buffers
//! that feed them to `petramond_world::exposure::BodyExposure::tick` for
//! players and mobs alike.

use crate::world::World;
use petramond_math::math::Vec3;
use petramond_world::condition::BodyConditions;
use petramond_world::exposure::{BodyExposure, ExposureDamage};
use petramond_world::{fluid::FluidDef, fluid_math::fluid_height};

const CONTACT_EPS: f32 = 1e-4;

/// Buffers reused across exposure ticks, so ticking a body allocates nothing.
#[derive(Default)]
pub(crate) struct ExposureScratch {
    touched: Vec<&'static FluidDef>,
    /// The damage the last [`tick`](Self::tick) found due.
    pub damage: Vec<ExposureDamage>,
}

impl ExposureScratch {
    /// Advance `exposure` one tick against the fluids `boxes` touch, replacing
    /// [`damage`](Self::damage) with what is due.
    pub fn tick(
        &mut self,
        world: &World,
        boxes: impl IntoIterator<Item = (Vec3, Vec3)>,
        exposure: &mut BodyExposure,
    ) {
        self.damage.clear();
        let known = gather_touched_fluids(world, boxes, &mut self.touched);
        exposure.tick(
            known.then_some(self.touched.as_slice()),
            petramond_world::condition::defs(),
            &mut self.damage,
        );
    }
}

/// Fill `out` with every fluid overlapping the boxes, sorted and unique by
/// block id. `false` when terrain under a box is not final, leaving `out`
/// meaningless. A fluid's surface excludes the empty space above a thin flow,
/// and contained fluids count like any other.
fn gather_touched_fluids(
    world: &World,
    boxes: impl IntoIterator<Item = (Vec3, Vec3)>,
    out: &mut Vec<&'static FluidDef>,
) -> bool {
    out.clear();
    for (min, max) in boxes {
        let lo = (min + Vec3::splat(CONTACT_EPS)).floor().as_ivec3();
        let hi = (max - Vec3::splat(CONTACT_EPS)).floor().as_ivec3();
        for y in lo.y..=hi.y {
            for z in lo.z..=hi.z {
                for x in lo.x..=hi.x {
                    let Some(block) = world.block_if_stream_final(x, y, z) else {
                        return false;
                    };
                    let Some(def) = block.fluid().and_then(|f| f.fluid_def()) else {
                        continue;
                    };
                    let Some(above) = world.block_if_stream_final(x, y + 1, z) else {
                        return false;
                    };
                    let height = fluid_height(world.fluid_meta_world(x, y, z), above, def.block);
                    if min.y + CONTACT_EPS < y as f32 + height {
                        out.push(def);
                    }
                }
            }
        }
    }
    out.sort_by_key(|f| f.block.id());
    out.dedup_by_key(|f| f.block.id());
    true
}

/// [`gather_touched_fluids`] into a fresh list; `None` for unknown terrain.
#[cfg(test)]
pub(crate) fn touched_fluids(
    world: &World,
    boxes: impl IntoIterator<Item = (Vec3, Vec3)>,
) -> Option<Vec<&'static FluidDef>> {
    let mut out = Vec::new();
    gather_touched_fluids(world, boxes, &mut out).then_some(out)
}

/// The ABI view of a body's active conditions, in condition-id order.
pub(crate) fn condition_data(conditions: &BodyConditions) -> Vec<mod_api::ConditionData> {
    conditions
        .active()
        .iter()
        .map(|c| mod_api::ConditionData {
            condition: mod_api::ConditionId(c.condition.0),
            stage: c.stage(),
            remaining: c.remaining(),
            elapsed: c.elapsed(),
        })
        .collect()
}
