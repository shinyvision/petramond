use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Aabb;
use petramond_world::collision::{self, DynBox};

use super::{body_boxes, def, resolve_body_motion, Instance, MobSize};
use crate::world::ServerWorld;

pub(crate) fn probe(
    world: &ServerWorld,
    mob: &Instance,
    offsets: &[[f32; 2]],
    max_drop: f32,
) -> Vec<bool> {
    if !mob.on_ground() || mob.entombed() {
        return vec![false; offsets.len()];
    }
    let d = def(mob.kind);
    let cursor = world.cursor();
    let boxes = |x, y, z| cursor.collision_boxes(IVec3::new(x, y, z));
    let obstacles = world.mobs().solid_obstacles();
    let safe = |min: [f64; 3], max: [f64; 3]| {
        let mut lo = min.map(|v| (v + 1e-5).floor() as i32);
        lo[1] = (min[1] - 1e-5).floor() as i32;
        let hi = max.map(|v| (v - 1e-5).floor() as i32);
        (lo[0]..=hi[0]).all(|x| {
            (lo[1]..=hi[1]).all(|y| (lo[2]..=hi[2]).all(|z| cursor.cell_final(IVec3::new(x, y, z))))
        }) && !super::nav::body_in_hazard(&cursor, min, max, d.tolerates.blocks)
    };
    offsets
        .iter()
        .map(|&offset| {
            walk_clear(
                mob.pos,
                mob.yaw,
                d.size,
                mob.id(),
                offset,
                max_drop,
                &boxes,
                &obstacles,
                &safe,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn walk_clear(
    start: WorldPos,
    yaw: f32,
    size: MobSize,
    id: u64,
    offset: [f32; 2],
    max_drop: f32,
    boxes: &impl Fn(i32, i32, i32) -> &'static [Aabb],
    obstacles: &[DynBox],
    safe: &impl Fn([f64; 3], [f64; 3]) -> bool,
) -> bool {
    // Check the straight leg itself: a path to its endpoint could take an unrelated detour.
    let steps = (offset[0].hypot(offset[1]) / 0.1).ceil().max(1.0) as usize;
    let delta = [offset[0] / steps as f32, 0.0, offset[1] / steps as f32];
    let mut pos = start;
    let mut escape = collision::EscapeRoute::default();
    for _ in 0..steps {
        let (moved, _, _, healed) = resolve_body_motion(
            pos,
            yaw,
            size,
            delta,
            1.0,
            collision::STEP_HEIGHT,
            true,
            &mut escape,
            boxes,
            obstacles,
            obstacles,
            id,
        );
        if healed != [0.0; 3]
            || (moved[0] - delta[0]).abs() > 1e-4
            || (moved[2] - delta[2]).abs() > 1e-4
        {
            return false;
        }
        pos += Vec3::from_array(moved);
        let down = -(max_drop + (pos.y - start.y) as f32 + 0.01).max(0.01);
        let landing = body_boxes(pos, yaw, size)
            .map(|(min, max)| collision::sweep_axis_dyn(min, max, 1, down, boxes, obstacles, id))
            .fold(down, f32::max);
        if landing <= down + 1e-4 {
            return false;
        }
        pos.y += f64::from(landing);
        if start.y - pos.y > f64::from(max_drop) + 1e-4 {
            return false;
        }
        if !body_boxes(pos, yaw, size).all(|(min, max)| safe(min, max)) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests;
