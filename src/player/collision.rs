use super::state::Player;
use crate::block::Aabb;
#[cfg(test)]
use crate::mathh::IVec3;

/// Boundary epsilon in world units. The AABB is shrunk by this on every side before
/// its float edges are compared to block faces, so an edge flush on a voxel boundary
/// — or a hair off from float error — is not treated as overlapping the neighbour.
#[cfg(test)]
const EPS: f64 = 1e-4;

/// One whole cell — a full cube's collision shape. Used by the test-only bool
/// [`Player::sweep`] adapter to turn each solid cell into a box for the swept-AABB.
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
    /// Move along one axis by `delta`, stopping at the first FULL-CUBE solid voxel the
    /// AABB would enter. A thin adapter over [`sweep_boxes`](Self::sweep_boxes) — each
    /// solid cell becomes one unit box — kept so the low-level collision tests can
    /// drive the sweep with a simple bool predicate. Returns true if a block was hit.
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

    /// Move along one axis by `delta`, stopping where the player's AABB first meets a
    /// block collision box. `boxes(x,y,z)` returns the cell-local collision AABBs of
    /// the block at that cell (empty = no collision); see
    /// [`Block::collision_boxes`](crate::block::Block::collision_boxes).
    ///
    /// A swept-AABB over those boxes: it scans every cell the body sweeps through
    /// (nearest wins, so it never tunnels) and, for each box, clamps travel on `axis`
    /// to the box's near face — but only when the player actually overlaps that box on
    /// the two OTHER axes. The cross-axis overlap is the whole point of a *shape*
    /// system: it lets you stand on a half-height block, or walk past the empty margin
    /// of an inset one, instead of colliding with the whole cell. A full cube is just
    /// the single-unit-box case and resolves identically to the old cell sweep.
    /// Returns true if movement was blocked (clamped short of `delta`).
    pub(super) fn sweep_boxes<F>(&mut self, axis: Axis, delta: f32, boxes: &F) -> bool
    where
        F: Fn(i32, i32, i32) -> &'static [Aabb],
    {
        self.sweep_boxes_dyn(axis, delta, boxes, &[])
    }

    /// [`sweep_boxes`](Self::sweep_boxes) that also resolves against dynamic
    /// boxes (solid entities). Returns true if movement was blocked.
    pub(super) fn sweep_boxes_dyn<F>(
        &mut self,
        axis: Axis,
        delta: f32,
        boxes: &F,
        obstacles: &[crate::collision::DynBox],
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
        // The swept-AABB itself is the shared, model-aware primitive — every moving entity
        // resolves against the same `collision::sweep_axis`; the player just applies the
        // travel to its body and reports whether it was clamped short.
        let travel = crate::collision::sweep_axis_dyn(
            [mn.x, mn.y, mn.z],
            [mx.x, mx.y, mx.z],
            ai,
            delta,
            boxes,
            obstacles,
            crate::collision::NOT_AN_ENTITY,
        );
        match axis {
            Axis::X => self.pos.x += travel,
            Axis::Y => self.pos.y += travel,
            Axis::Z => self.pos.z += travel,
        }
        travel.abs() + 1e-6 < delta.abs()
    }

    /// Does the player's AABB overlap the unit cube at integer cell `b`? Test-only
    /// probe kept in lock-step with the cell set [`Player::sweep`] resolves against.
    #[cfg(test)]
    pub fn intersects_block(&self, b: IVec3) -> bool {
        let min = self.aabb_min();
        let max = self.aabb_max();
        (cell_min(min.x)..=cell_max(max.x)).contains(&b.x)
            && (cell_min(min.y)..=cell_max(max.y)).contains(&b.y)
            && (cell_min(min.z)..=cell_max(max.z)).contains(&b.z)
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
