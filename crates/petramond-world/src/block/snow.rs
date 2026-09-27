use super::Block;
use glam::IVec3;

pub const SNOW_COVER_REACH: i32 = 1;
pub const SNOW_BEDDING_REACH: i32 = 1;

pub fn snow_cover_at(pos: IVec3, block_at: impl Fn(IVec3) -> Block) -> Option<Block> {
    let block = block_at(pos);
    if block.is_snow_cover() {
        return Some(block);
    }
    if !block.is_snow_bedded() {
        return None;
    }
    let r = SNOW_BEDDING_REACH;
    [(r, 0), (-r, 0), (0, r), (0, -r)]
        .into_iter()
        .map(|(dx, dz)| block_at(pos + IVec3::new(dx, 0, dz)))
        .find(|b| b.is_snow_cover())
}
