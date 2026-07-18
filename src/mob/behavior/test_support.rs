//! The one test builder for [`AiCtx`]: a neutral context each test customizes
//! per field, so adding a perception fact costs one default here instead of a
//! struct-literal edit in every behavior/brain/manager test module.

use crate::mathh::{IVec3, Vec3};
use crate::mob::brain::AiCtx;
use crate::mob::MobRng;
use crate::world::World;

/// A neutral, idle, dry context: a small mob at the origin, the nearest
/// player at the origin too, no perception input of any kind. Tests set only
/// the fields they vary (the fields are `pub`).
pub(crate) fn ctx<'a>(world: &'a World, rng: &'a mut MobRng) -> AiCtx<'a> {
    AiCtx {
        mob_id: 1,
        pos: Vec3::ZERO,
        cell: IVec3::ZERO,
        yaw: 0.0,
        head_height: 0.7,
        half_width: 0.25,
        world,
        player_id: Default::default(),
        player_pos: Vec3::ZERO,
        player_sneaking: false,
        player_held: None,
        players: &[],
        noises: &[],
        contacts: &[],
        target: None,
        attacker: None,
        nav_idle: true,
        in_water: false,
        head: 1,
        idle_anims: &[],
        mob_index: 0,
        mobs: &[],
        rng,
    }
}

/// [`ctx`] positioned at `pos` (cell derived from the feet).
pub(crate) fn ctx_at<'a>(world: &'a World, rng: &'a mut MobRng, pos: Vec3) -> AiCtx<'a> {
    let mut c = ctx(world, rng);
    c.pos = pos;
    c.cell = crate::mathh::voxel_at(pos);
    c
}
