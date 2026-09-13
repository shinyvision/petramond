//! The world's horizontal edge.
//!
//! Positions are f64 and rendering is origin-relative, so precision holds far
//! out; what runs out first is `i32` block coordinates. The border sits far
//! enough inside `i32` that a block coordinate plus any view, reach or
//! structure radius never overflows.

use petramond_math::world_pos::WorldPos;

/// Columns with `x` or `z` outside `-WORLD_BORDER..WORLD_BORDER` are outside the world.
pub const WORLD_BORDER: i32 = 1 << 30;

/// Whether column `(wx, wz)` lies inside the world.
#[inline]
pub fn contains_column(wx: i32, wz: i32) -> bool {
    (-WORLD_BORDER..WORLD_BORDER).contains(&wx) && (-WORLD_BORDER..WORLD_BORDER).contains(&wz)
}

/// `p` pulled horizontally inside the border, a block clear of the wall so a
/// body placed there does not start embedded in it.
pub fn clamp(p: WorldPos) -> WorldPos {
    let limit = f64::from(WORLD_BORDER - 1);
    WorldPos::new(p.x.clamp(-limit, limit), p.y, p.z.clamp(-limit, limit))
}
