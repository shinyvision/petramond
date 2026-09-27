use super::state::Player;
#[cfg(test)]
use petramond_math::math::IVec3;
use petramond_world::block::Aabb;

#[cfg(test)]
const EPS: f64 = 1e-4;

#[cfg(test)]
const FULL_CUBE: &[Aabb] = &[Aabb {
    min: [0.0, 0.0, 0.0],
    max: [1.0, 1.0, 1.0],
}];

#[derive(Copy, Clone)]
pub(super) enum Axis {
    X,
    Y,
    Z,
}

impl Player {
    #[cfg(test)]
    pub(super) fn sweep<F: Fn(i32, i32, i32) -> bool>(
        &mut self,
        axis: Axis,
        delta: f32,
        solid: &F,
    ) -> bool {
        self.sweep_boxes(axis, delta, &|x, y, z| {
            if solid(x, y, z) {
                FULL_CUBE
            } else {
                &[]
            }
        })
    }

    pub(super) fn sweep_boxes<F>(&mut self, axis: Axis, delta: f32, boxes: &F) -> bool
    where
        F: Fn(i32, i32, i32) -> &'static [Aabb],
    {
        self.sweep_boxes_dyn(axis, delta, boxes, &[])
    }

    pub(super) fn sweep_boxes_dyn<F>(
        &mut self,
        axis: Axis,
        delta: f32,
        boxes: &F,
        obstacles: &[petramond_world::collision::DynBox],
    ) -> bool
    where
        F: Fn(i32, i32, i32) -> &'static [Aabb],
    {
        let mn = self.aabb_min();
        let mx = self.aabb_max();
        let ai = match axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        };
        let travel = petramond_world::collision::sweep_axis_dyn(
            mn,
            mx,
            ai,
            delta,
            boxes,
            obstacles,
            petramond_world::collision::NOT_AN_ENTITY,
        );
        match axis {
            Axis::X => self.pos.x += f64::from(travel),
            Axis::Y => self.pos.y += f64::from(travel),
            Axis::Z => self.pos.z += f64::from(travel),
        }
        travel.abs() + 1e-6 < delta.abs()
    }

    #[cfg(test)]
    pub fn intersects_block(&self, b: IVec3) -> bool {
        let min = self.aabb_min();
        let max = self.aabb_max();
        (cell_min(min[0] as f32)..=cell_max(max[0] as f32)).contains(&b.x)
            && (cell_min(min[1] as f32)..=cell_max(max[1] as f32)).contains(&b.y)
            && (cell_min(min[2] as f32)..=cell_max(max[2] as f32)).contains(&b.z)
    }
}

#[cfg(test)]
#[inline]
fn cell_min(edge: f32) -> i32 {
    (edge.next_up() as f64 + EPS).floor() as i32
}

#[cfg(test)]
#[inline]
fn cell_max(edge: f32) -> i32 {
    (edge.next_down() as f64 - EPS).floor() as i32
}
