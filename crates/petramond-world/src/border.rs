use petramond_math::world_pos::WorldPos;

pub const WORLD_BORDER: i32 = 1 << 30;

#[inline]
pub fn contains_column(wx: i32, wz: i32) -> bool {
    (-WORLD_BORDER..WORLD_BORDER).contains(&wx) && (-WORLD_BORDER..WORLD_BORDER).contains(&wz)
}

pub fn clamp(p: WorldPos) -> WorldPos {
    let limit = f64::from(WORLD_BORDER - 1);
    WorldPos::new(p.x.clamp(-limit, limit), p.y, p.z.clamp(-limit, limit))
}
