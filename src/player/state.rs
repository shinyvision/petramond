use crate::block::Block;
use crate::mathh::Vec3;
use crate::world::World;

/// Half the horizontal width (box is 0.6 wide on x and z).
pub const HALF_W: f32 = 0.3;
/// Full body height.
pub const HEIGHT: f32 = 1.8;
/// Eye height above the feet (matches Minecraft's 1.62).
pub const EYE: f32 = 1.62;
/// Largest physics sub-step; `app` splits a frame's `dt` into chunks this size
/// so a long stall can't make one update step move (and tunnel) too far.
pub const DT_MAX: f32 = 0.05;

/// Per-frame movement intent, in world space.
#[derive(Copy, Clone, Default)]
pub struct Input {
    /// Horizontal wish direction (unit length, or zero). Y is ignored.
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
}

pub struct Player {
    /// Feet centre (see module docs).
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    /// True between a jump take-off and the next blocked vertical sweep (landing
    /// or head-bonk). Gates the apex easing so only a genuine jump arc is
    /// softened — walking off a ledge or bonking a ceiling falls at full gravity.
    pub(super) jumping: bool,
}

impl Player {
    pub fn new(feet: Vec3) -> Self {
        Self {
            pos: feet,
            vel: Vec3::ZERO,
            on_ground: false,
            jumping: false,
        }
    }

    /// Eye position (camera origin).
    #[inline]
    pub fn eye(&self) -> Vec3 {
        Vec3::new(self.pos.x, self.pos.y + EYE, self.pos.z)
    }

    /// AABB min corner.
    #[inline]
    pub(super) fn aabb_min(&self) -> Vec3 {
        Vec3::new(self.pos.x - HALF_W, self.pos.y, self.pos.z - HALF_W)
    }

    /// AABB max corner.
    #[inline]
    pub(super) fn aabb_max(&self) -> Vec3 {
        Vec3::new(
            self.pos.x + HALF_W,
            self.pos.y + HEIGHT,
            self.pos.z + HALF_W,
        )
    }

    #[inline]
    pub(super) fn solid_world(world: &World, x: i32, y: i32, z: i32) -> bool {
        Block::from_id(world.chunk_block(x, y, z)).is_solid()
    }

    /// True if every chunk column the horizontal AABB overlaps is loaded. The
    /// caller gates physics on this (once per frame) so the player can't fall
    /// through terrain that hasn't generated yet (spawn, or running past the
    /// load frontier). Column membership can't change within a frame, so this
    /// need not be re-checked per sub-step.
    pub fn columns_loaded(&self, world: &World) -> bool {
        let cx0 = (self.pos.x - HALF_W).floor() as i32 >> 4;
        let cx1 = (self.pos.x + HALF_W).floor() as i32 >> 4;
        let cz0 = (self.pos.z - HALF_W).floor() as i32 >> 4;
        let cz1 = (self.pos.z + HALF_W).floor() as i32 >> 4;
        for cx in cx0..=cx1 {
            for cz in cz0..=cz1 {
                if !world.chunk_loaded(cx, cz) {
                    return false;
                }
            }
        }
        true
    }
}
