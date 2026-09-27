use petramond_math::world_pos::WorldPos;
use serde::{Deserialize, Serialize};

use crate::net::remap::{IdRemap, Remap};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewCue {
    pub at: f64,
    pub pos: WorldPos,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
    pub shake: CueShake,
    pub motion: CueMotion,
    pub hands: [CueHand; 2],
    pub hotbar: u8,
    pub animator: crate::player::AnimatorClaims,
    pub events: Vec<(crate::player::RigId, u16)>,
}

impl ViewCue {
    pub fn presents_as(&self, other: &ViewCue) -> bool {
        self.pos == other.pos
            && self.yaw == other.yaw
            && self.pitch == other.pitch
            && self.roll == other.roll
            && self.fov_y == other.fov_y
            && self.shake == other.shake
            && self.motion == other.motion
            && self.hands == other.hands
            && self.hotbar == other.hotbar
            && self.animator == other.animator
            && self.events == other.events
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CueShake {
    pub look: [f32; 2],
    pub hand: [f32; 2],
    pub flash: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CueMotion {
    pub speed: f32,
    pub forward: f32,
    pub strafe: f32,
    pub vertical: f32,
    pub grounded: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    pub swimming: bool,
    pub climbing: bool,
    pub pitch: f32,
    pub yaw_rate: f32,
    pub pitch_rate: f32,
    pub stride: f32,
    pub stride_weight: f32,
    pub hurt: f32,
    pub target: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CueHand {
    pub item: Option<u16>,
    pub display: Option<u16>,
    pub data: Option<Vec<u8>>,
    pub mining: bool,
    pub eating: Option<f32>,
    pub pose: Option<mod_api::HeldPose>,
}

impl Remap for ViewCue {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let ViewCue {
            at: _,
            pos: _,
            yaw: _,
            pitch: _,
            roll: _,
            fov_y: _,
            shake: _,
            motion: _,
            hands,
            hotbar: _,
            animator,
            events,
        } = self;
        for hand in hands {
            hand.remap(map);
        }
        events.retain_mut(|(rig, event)| map.remap_animator_event(rig, event));
        animator.remap(map)
    }
}

impl Remap for CueHand {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let CueHand {
            item,
            display,
            data: _,
            mining: _,
            eating: _,
            pose: _,
        } = self;
        map.optional_item(item);
        map.optional_item(display);
        true
    }
}
