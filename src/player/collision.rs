use super::state::{Player, HALF_W, HEIGHT};
use crate::mathh::IVec3;

/// Boundary epsilon: the AABB is shrunk by this on every side before its float
/// edges are turned into integer cell indices, so an edge flush on a voxel
/// boundary — or a hair off from float error — is *not* treated as occupying the
/// neighbouring cell. Applied symmetrically (see `lo`/`hi` in `sweep`) for a
/// consistent cell set per axis regardless of approach direction or world
/// position. (Past a few thousand blocks one f32 ULP exceeds EPS, so it stops
/// biting and collisions degrade — phantom blocks and tunnelling return; the
/// inherent limit of an f32 voxel world, reached only far outside normal play.)
const EPS: f32 = 1e-4;

#[derive(Copy, Clone)]
pub(super) enum Axis {
    X,
    Y,
    Z,
}

impl Player {
    /// Move along one axis by `delta`, stopping at the first solid voxel slice
    /// the AABB would enter. Scans *every* cell slice swept (nearest first), so
    /// it never tunnels regardless of `delta`. Returns true if a block was hit.
    pub(super) fn sweep<F: Fn(i32, i32, i32) -> bool>(
        &mut self,
        axis: Axis,
        delta: f32,
        solid: &F,
    ) -> bool {
        if delta == 0.0 {
            return false;
        }
        let min = self.aabb_min();
        let max = self.aabb_max();
        // Cell index of a min edge (inclusive) and a max edge (exclusive-ish).
        // Both shrink the box by EPS so an edge sitting on a voxel boundary — or
        // a hair off it from float error (e.g. 1.3 - 0.3 = 0.99999994) — yields a
        // consistent cell set: a flush min edge does not pull in the cell below,
        // and a flush max edge does not pull in the cell above.
        let lo = |a: f32| (a + EPS).floor() as i32;
        let hi = |b: f32| (b - EPS).floor() as i32;

        match axis {
            Axis::X => {
                let (a0, a1) = (lo(min.y), hi(max.y));
                let (b0, b1) = (lo(min.z), hi(max.z));
                if delta > 0.0 {
                    let to = hi(max.x + delta);
                    for c in (hi(max.x) + 1)..=to {
                        if Self::slice_solid(solid, Axis::X, c, a0, a1, b0, b1) {
                            self.pos.x = c as f32 - HALF_W;
                            return true;
                        }
                    }
                } else {
                    let to = lo(min.x + delta);
                    for c in ((to)..=(lo(min.x) - 1)).rev() {
                        if Self::slice_solid(solid, Axis::X, c, a0, a1, b0, b1) {
                            self.pos.x = (c + 1) as f32 + HALF_W;
                            return true;
                        }
                    }
                }
                self.pos.x += delta;
                false
            }
            Axis::Z => {
                let (a0, a1) = (lo(min.x), hi(max.x));
                let (b0, b1) = (lo(min.y), hi(max.y));
                if delta > 0.0 {
                    let to = hi(max.z + delta);
                    for c in (hi(max.z) + 1)..=to {
                        if Self::slice_solid(solid, Axis::Z, c, a0, a1, b0, b1) {
                            self.pos.z = c as f32 - HALF_W;
                            return true;
                        }
                    }
                } else {
                    let to = lo(min.z + delta);
                    for c in ((to)..=(lo(min.z) - 1)).rev() {
                        if Self::slice_solid(solid, Axis::Z, c, a0, a1, b0, b1) {
                            self.pos.z = (c + 1) as f32 + HALF_W;
                            return true;
                        }
                    }
                }
                self.pos.z += delta;
                false
            }
            Axis::Y => {
                let (a0, a1) = (lo(min.x), hi(max.x));
                let (b0, b1) = (lo(min.z), hi(max.z));
                if delta > 0.0 {
                    let to = hi(max.y + delta);
                    for c in (hi(max.y) + 1)..=to {
                        if Self::slice_solid(solid, Axis::Y, c, a0, a1, b0, b1) {
                            self.pos.y = c as f32 - HEIGHT;
                            return true;
                        }
                    }
                } else {
                    let to = lo(min.y + delta);
                    for c in ((to)..=(lo(min.y) - 1)).rev() {
                        if Self::slice_solid(solid, Axis::Y, c, a0, a1, b0, b1) {
                            self.pos.y = (c + 1) as f32;
                            return true;
                        }
                    }
                }
                self.pos.y += delta;
                false
            }
        }
    }

    /// Is any voxel in the AABB's cross-section at slice `c` (along `axis`)
    /// solid? `(a0,a1)` and `(b0,b1)` are the inclusive cell ranges of the two
    /// fixed axes (order: X→(Y,Z), Z→(X,Y), Y→(X,Z)).
    #[inline]
    fn slice_solid<F: Fn(i32, i32, i32) -> bool>(
        solid: &F,
        axis: Axis,
        c: i32,
        a0: i32,
        a1: i32,
        b0: i32,
        b1: i32,
    ) -> bool {
        for a in a0..=a1 {
            for b in b0..=b1 {
                let hit = match axis {
                    Axis::X => solid(c, a, b),
                    Axis::Z => solid(a, b, c),
                    Axis::Y => solid(a, c, b),
                };
                if hit {
                    return true;
                }
            }
        }
        false
    }

    /// Does the player's AABB overlap the unit cube at integer cell `b`? The box
    /// is shrunk by `EPS` on every side — the same cell set [`Player::sweep`]
    /// resolves against — so a block merely flush against the player (touching a
    /// face, exactly or a hair off from float error) does *not* count. Keeping
    /// this in lock-step with `sweep` matters: it gates block placement, so you
    /// can place a block in exactly the cells the collision sweep lets you stand
    /// beside (no "can't place where I clearly fit", no "placed inside myself").
    pub fn intersects_block(&self, b: IVec3) -> bool {
        let min = self.aabb_min();
        let max = self.aabb_max();
        let (bx, by, bz) = (b.x as f32, b.y as f32, b.z as f32);
        min.x + EPS < bx + 1.0
            && max.x - EPS > bx
            && min.y + EPS < by + 1.0
            && max.y - EPS > by
            && min.z + EPS < bz + 1.0
            && max.z - EPS > bz
    }
}
