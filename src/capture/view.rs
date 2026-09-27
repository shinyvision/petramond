//! The locally presented first-person view, as a frame's `View` piece
//! carries it: exactly what the capturing client's first person consumed on
//! that frame, predicted locally and so carried by no replicated row.

use petramond_math::world_pos::WorldPos;
use serde::{Deserialize, Serialize};

use crate::net::remap::{IdRemap, Remap};

/// Exactly the inputs one frame's first-person presentation consumed: the
/// eye as it stood (bob included), its field of view (speed coupling
/// included), the hurt shake on top, the body motion the viewmodel sways
/// with, and both hands as the local predictor presented them — whatever the
/// frame presented (a claimed camera or third person still captures the eye).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewCue {
    /// The replicated tick the frame presented, fractional: the tick the
    /// render interpolated toward, less one, plus its alpha — the scale a
    /// presentation's position is on.
    pub at: f64,
    pub pos: WorldPos,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
    pub shake: CueShake,
    pub motion: CueMotion,
    /// `[main, off]`.
    pub hands: [CueHand; 2],
    /// The selected hotbar slot the HUD showed.
    pub hotbar: u8,
    /// The first-person rig's claims as presented.
    pub animator: crate::player::AnimatorClaims,
    /// Graph events fired on the first-person rigs that frame.
    pub events: Vec<(crate::player::RigId, u16)>,
}

impl ViewCue {
    /// Whether `other` presents the same view: every field but the time it
    /// was presented at.
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

/// The hurt shake as the frame wore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CueShake {
    /// Look offset `[yaw, pitch]`, radians (zero when the player turned
    /// screen shake off).
    pub look: [f32; 2],
    /// Hand screen offset, NDC.
    pub hand: [f32; 2],
    /// The red edge-vignette strength, `[0, 1]`.
    pub flash: f32,
}

/// The body motion the first-person viewmodel read that frame.
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
    /// What the crosshair rested on: 0 nothing, 1 a block, 2 a creature.
    pub target: u8,
}

/// One hand of a [`ViewCue`], in the stream's id vocabulary (item ids remap
/// by name).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CueHand {
    pub item: Option<u16>,
    pub display: Option<u16>,
    /// The held stack's canonical instance data.
    pub data: Option<Vec<u8>>,
    pub mining: bool,
    pub eating: Option<f32>,
    pub pose: Option<mod_api::HeldPose>,
}

/// A recorded first-person view, in its stream's id vocabulary: an unknown
/// hand item reads as an empty hand, and rig claims and fired events this
/// process lacks drop alone.
impl Remap for ViewCue {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let ViewCue {
            at: _,
            pos: _,
            yaw: _,
            pitch: _,
            roll: _,
            fov_y: _,
            // Offsets and the body's motion scalars.
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
            // Instance data is canonical bytes, not registry ids.
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
