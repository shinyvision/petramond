use crate::world::WorldData;
use petramond_math::math::IVec3;
use petramond_world::light::BlockLight6;

pub fn break_light(world: &WorldData, pos: IVec3, normal: Option<IVec3>) -> (u8, BlockLight6) {
    let at = |c: IVec3| world.dynamic_light_at_world(c.x, c.y, c.z);
    if let Some(n) = normal {
        return at(pos + n);
    }

    [
        IVec3::X,
        -IVec3::X,
        IVec3::Y,
        -IVec3::Y,
        IVec3::Z,
        -IVec3::Z,
    ]
    .into_iter()
    .map(|n| at(pos + n))
    .max_by_key(|&(sky, block)| sky.max(block.luminance() as u8))
    .unwrap_or((63, BlockLight6::DARK))
}
