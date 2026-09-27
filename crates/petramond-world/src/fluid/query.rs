use super::{Buoyancy, FluidCurrent, Immersion};
use crate::{
    block::Block,
    fluid_math::{fluid_height, is_source, surface_flow_dir},
    mathh::{IVec3, Vec3},
    world::{SectionCursor, WorldData},
};
use petramond_math::world_pos::WorldPos;

impl WorldData {
    pub fn is_fluid_source_world(&self, pos: IVec3, fluid: Block) -> bool {
        self.physics_block(pos.x, pos.y, pos.z) == fluid
            && is_source(self.fluid_meta_world(pos.x, pos.y, pos.z))
    }

    pub fn fluid_at_point(&self, point: WorldPos) -> Option<Immersion> {
        let sample = fluid_in_cell(&self.cursor(), point.block())?;
        (point.y < f64::from(sample.surface_y)).then_some(sample)
    }

    pub fn body_fluid(&self, feet: WorldPos, height: f32, buoyancy: Buoyancy) -> Option<Immersion> {
        let cur = self.cursor();
        if buoyancy == Buoyancy::Surface {
            let cell = feet.block();
            return fluid_surface_at(&cur, cell)
                .or_else(|| fluid_surface_at(&cur, cell - IVec3::Y));
        }
        let x = feet.x.floor() as i32;
        let z = feet.z.floor() as i32;
        for y in feet.y.floor() as i32..=(feet.y + f64::from(height)).floor() as i32 {
            let Some(sample) = fluid_in_cell(&cur, IVec3::new(x, y, z)) else {
                continue;
            };
            let probe = feet.y + f64::from(sample.fluid.motion.probe_height(height));
            if probe >= f64::from(y) && probe < f64::from(sample.surface_y) {
                return Some(sample);
            }
        }
        None
    }

    pub fn fluid_surface_at(&self, cell: IVec3) -> Option<Immersion> {
        fluid_surface_at(&self.cursor(), cell)
    }

    pub fn fluid_flow_dir_at(&self, wx: i32, wy: i32, wz: i32, fluid: Block) -> Vec3 {
        let cur = self.cursor();
        let block_at = |x: i32, y: i32, z: i32| {
            let block = cur.physics_block(IVec3::new(x, y, z));
            block.fluid().unwrap_or(block)
        };
        let fluid_at = |x: i32, y: i32, z: i32| -> Option<f32> {
            if block_at(x, y, z) != fluid {
                return None;
            }
            Some(fluid_height(
                cur.fluid_meta(IVec3::new(x, y, z)),
                block_at(x, y + 1, z),
                fluid,
            ))
        };
        let still_at = |x: i32, y: i32, z: i32| {
            block_at(x, y, z) == fluid && is_source(cur.fluid_meta(IVec3::new(x, y, z)))
        };
        surface_flow_dir(wx, wy, wz, fluid, &block_at, &fluid_at, &still_at)
    }

    pub fn fluid_current_at(&self, p: WorldPos) -> FluidCurrent {
        let Some(sample) = self.fluid_at_point(p) else {
            return FluidCurrent::NONE;
        };
        let fluid = sample.fluid;
        if fluid.current.speed <= 0.0 {
            return FluidCurrent::NONE;
        }
        let c = p.block();
        FluidCurrent {
            velocity: self.fluid_flow_dir_at(c.x, c.y, c.z, fluid.block) * fluid.current.speed,
            accel: fluid.current.accel,
        }
    }

    pub fn body_current(
        &self,
        feet: WorldPos,
        height: f32,
        immersion: Option<Immersion>,
    ) -> FluidCurrent {
        sample_body_current(feet, height, immersion, |p| self.fluid_current_at(p))
    }
}

fn fluid_surface_at(cur: &SectionCursor<'_>, cell: IVec3) -> Option<Immersion> {
    let mut sample = fluid_in_cell(cur, cell)?;
    let mut top = cell;
    while cur.physics_block(top + IVec3::Y).fluid() == Some(sample.fluid.block) {
        top.y += 1;
    }
    sample.surface_y = top.y as f32
        + fluid_height(
            cur.fluid_meta(top),
            cur.physics_block(top + IVec3::Y),
            sample.fluid.block,
        );
    Some(sample)
}

fn fluid_in_cell(cur: &SectionCursor<'_>, cell: IVec3) -> Option<Immersion> {
    let fluid = cur.physics_block(cell).fluid()?.fluid_def()?;
    let surface_y = cell.y as f32
        + fluid_height(
            cur.fluid_meta(cell),
            cur.physics_block(cell + IVec3::Y),
            fluid.block,
        );
    Some(Immersion { fluid, surface_y })
}

pub fn sample_body_current(
    feet: WorldPos,
    height: f32,
    immersion: Option<Immersion>,
    at: impl Fn(WorldPos) -> FluidCurrent,
) -> FluidCurrent {
    let probe = immersion.map_or(0.05, |s| s.fluid.motion.probe_height(height));
    let current = at(feet + Vec3::Y * probe);
    if current.velocity.length_squared() > 0.0 {
        current
    } else {
        at(feet + Vec3::Y * 0.05)
    }
}
