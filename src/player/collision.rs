use super::state::Player;
use crate::block::Aabb;
use crate::mathh::IVec3;

/// Boundary epsilon in world units. The AABB is shrunk by this on every side before
/// its float edges are compared to block faces, so an edge flush on a voxel boundary
/// — or a hair off from float error — is not treated as overlapping the neighbour.
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
        if delta == 0.0 {
            return false;
        }
        let mn = self.aabb_min();
        let mx = self.aabb_max();
        let pmin = [mn.x, mn.y, mn.z];
        let pmax = [mx.x, mx.y, mx.z];
        let eps = EPS as f32;
        let ai = match axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        };
        // Broad-phase cell ranges over the swept volume: the body, with the swept axis
        // extended by `delta` toward the move direction.
        let mut lo = [
            pmin[0].floor() as i32,
            pmin[1].floor() as i32,
            pmin[2].floor() as i32,
        ];
        let mut hi = [
            pmax[0].floor() as i32,
            pmax[1].floor() as i32,
            pmax[2].floor() as i32,
        ];
        if delta > 0.0 {
            hi[ai] = (pmax[ai] + delta).floor() as i32;
        } else {
            lo[ai] = (pmin[ai] + delta).floor() as i32;
        }

        let mut travel = delta;
        for cx in lo[0]..=hi[0] {
            for cy in lo[1]..=hi[1] {
                for cz in lo[2]..=hi[2] {
                    let cell = [cx as f32, cy as f32, cz as f32];
                    for b in boxes(cx, cy, cz) {
                        // Overlap on the two NON-swept axes (touching within EPS does
                        // not count — matches the old cell sweep's EPS-shrunk faces).
                        let mut cross = true;
                        for i in 0..3 {
                            if i == ai {
                                continue;
                            }
                            let wlo = cell[i] + b.min[i];
                            let whi = cell[i] + b.max[i];
                            if !(pmax[i] > wlo + eps && pmin[i] < whi - eps) {
                                cross = false;
                                break;
                            }
                        }
                        if !cross {
                            continue;
                        }
                        // Clamp travel so the leading face just meets the box's near
                        // face on the swept axis (only while the box is ahead of us).
                        if delta > 0.0 {
                            let allowed = (cell[ai] + b.min[ai]) - pmax[ai];
                            if allowed >= -eps {
                                travel = travel.min(allowed.max(0.0));
                            }
                        } else {
                            let allowed = (cell[ai] + b.max[ai]) - pmin[ai];
                            if allowed <= eps {
                                travel = travel.max(allowed.min(0.0));
                            }
                        }
                    }
                }
            }
        }

        match axis {
            Axis::X => self.pos.x += travel,
            Axis::Y => self.pos.y += travel,
            Axis::Z => self.pos.z += travel,
        }
        travel.abs() + 1e-6 < delta.abs()
    }

    /// Does the player's AABB overlap the unit cube at integer cell `b`? The box is
    /// shrunk by `EPS` on every side — the same cell set [`Player::sweep`] resolves
    /// against — so a block merely flush against the player (touching a face, exactly
    /// or a hair off from float error) does *not* count. Keeping this in lock-step
    /// with `sweep` matters: it gates block placement, so you can place a block in
    /// exactly the cells the collision sweep lets you stand beside (no "can't place
    /// where I clearly fit", no "placed inside myself").
    pub fn intersects_block(&self, b: IVec3) -> bool {
        let min = self.aabb_min();
        let max = self.aabb_max();
        (cell_min(min.x)..=cell_max(max.x)).contains(&b.x)
            && (cell_min(min.y)..=cell_max(max.y)).contains(&b.y)
            && (cell_min(min.z)..=cell_max(max.z)).contains(&b.z)
    }
}

#[inline]
fn cell_min(edge: f32) -> i32 {
    (edge.next_up() as f64 + EPS).floor() as i32
}

#[inline]
fn cell_max(edge: f32) -> i32 {
    (edge.next_down() as f64 - EPS).floor() as i32
}
