use super::{Buoyancy, FluidCurrent, Immersion};
use crate::{
    block::Block,
    fluid_math::{fluid_height, is_source, surface_flow_dir},
    mathh::{IVec3, Vec3},
    world::WorldData,
};

impl WorldData {
    /// The real fluid volume at a point; air above a thin flow is never immersed.
    pub fn fluid_at_point(&self, point: Vec3) -> Option<Immersion> {
        let cell = point.floor().as_ivec3();
        let sample = self.fluid_in_cell(cell)?;
        (point.y < sample.surface_y).then_some(sample)
    }

    /// Fluid response for a body, using its size and buoyancy mode.
    pub fn body_fluid(&self, feet: Vec3, height: f32, buoyancy: Buoyancy) -> Option<Immersion> {
        if buoyancy == Buoyancy::Surface {
            let cell = feet.floor().as_ivec3();
            return self
                .fluid_surface_at(cell)
                .or_else(|| self.fluid_surface_at(cell - IVec3::Y));
        }
        let x = feet.x.floor() as i32;
        let z = feet.z.floor() as i32;
        for y in feet.y.floor() as i32..=(feet.y + height).floor() as i32 {
            let Some(sample) = self.fluid_in_cell(IVec3::new(x, y, z)) else {
                continue;
            };
            let probe = feet.y + sample.fluid.motion.probe_height(height);
            if probe >= y as f32 && probe < sample.surface_y {
                return Some(sample);
            }
        }
        None
    }

    /// Top of the contiguous same-fluid column containing `cell`.
    pub fn fluid_surface_at(&self, cell: IVec3) -> Option<Immersion> {
        let mut sample = self.fluid_in_cell(cell)?;
        let mut top = cell;
        while self.physics_block(top.x, top.y + 1, top.z).fluid() == Some(sample.fluid.block) {
            top.y += 1;
        }
        sample.surface_y = top.y as f32
            + fluid_height(
                self.fluid_meta_world(top.x, top.y, top.z),
                self.physics_block(top.x, top.y + 1, top.z),
                sample.fluid.block,
            );
        Some(sample)
    }

    /// Horizontal direction a body drifts in when it overlaps this cell of
    /// `fluid`. Matches [`surface_flow_dir`], which the mesher also uses to face
    /// the flow texture.
    pub fn fluid_flow_dir_at(&self, wx: i32, wy: i32, wz: i32, fluid: Block) -> Vec3 {
        let block_at = |x: i32, y: i32, z: i32| {
            let block = self.physics_block(x, y, z);
            block.fluid().unwrap_or(block)
        };
        let fluid_at = |x: i32, y: i32, z: i32| -> Option<f32> {
            let p = IVec3::new(x, y, z);
            if block_at(p.x, p.y, p.z) != fluid {
                return None;
            }
            Some(fluid_height(
                self.fluid_meta_world(p.x, p.y, p.z),
                block_at(p.x, p.y + 1, p.z),
                fluid,
            ))
        };
        let still_at = |x: i32, y: i32, z: i32| {
            block_at(x, y, z) == fluid && is_source(self.fluid_meta_world(x, y, z))
        };
        surface_flow_dir(wx, wy, wz, fluid, &block_at, &fluid_at, &still_at)
    }

    /// Current at a point below the actual fluid surface, using the same
    /// gradient as the mesher and the fluid's configured speed/acceleration.
    pub fn fluid_current_at(&self, p: Vec3) -> FluidCurrent {
        let Some(sample) = self.fluid_at_point(p) else {
            return FluidCurrent::NONE;
        };
        let fluid = sample.fluid;
        if fluid.current.speed <= 0.0 {
            return FluidCurrent::NONE;
        }
        let c = p.floor().as_ivec3();
        FluidCurrent {
            velocity: self.fluid_flow_dir_at(c.x, c.y, c.z, fluid.block) * fluid.current.speed,
            accel: fluid.current.accel,
        }
    }

    /// Sample the swimming probe, then shallow wading contact when it has no current.
    pub fn body_current(
        &self,
        feet: Vec3,
        height: f32,
        immersion: Option<Immersion>,
    ) -> FluidCurrent {
        sample_body_current(feet, height, immersion, |p| self.fluid_current_at(p))
    }

    fn fluid_in_cell(&self, cell: IVec3) -> Option<Immersion> {
        let fluid = self
            .physics_block(cell.x, cell.y, cell.z)
            .fluid()?
            .fluid_def()?;
        let surface_y = cell.y as f32
            + fluid_height(
                self.fluid_meta_world(cell.x, cell.y, cell.z),
                self.physics_block(cell.x, cell.y + 1, cell.z),
                fluid.block,
            );
        Some(Immersion { fluid, surface_y })
    }
}

/// Sample a body's fluid current from any point-query source.
pub fn sample_body_current(
    feet: Vec3,
    height: f32,
    immersion: Option<Immersion>,
    at: impl Fn(Vec3) -> FluidCurrent,
) -> FluidCurrent {
    let probe = immersion.map_or(0.05, |s| s.fluid.motion.probe_height(height));
    let current = at(feet + Vec3::Y * probe);
    if current.velocity.length_squared() > 0.0 {
        current
    } else {
        at(feet + Vec3::Y * 0.05)
    }
}
