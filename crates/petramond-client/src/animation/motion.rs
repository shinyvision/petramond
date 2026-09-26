//! What the player animators read: the body's presented motion (no
//! simulation state), the local player's first-person motion, and the
//! claimed bone offsets composed over both.

use petramond_math::math::Tilt;

/// Swimming blend weights for the authored swim styles.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SwimBlend {
    pub grounded: f32,
    pub weight: f32,
    pub phase: f32,
    pub moving: f32,
    pub backward: f32,
    pub rising: f32,
}

/// Presentation weights for the authored locomotion styles. No simulation state.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LocomotionBlend {
    pub swim: SwimBlend,
    pub run: f32,
    pub backward: f32,
    pub airborne: f32,
    pub falling: f32,
    pub landing: f32,
    /// Signed lateral balance, positive toward the body's right.
    pub strafe: f32,
}

/// One player body's presented motion this frame — everything its pose and
/// its body animator read. Player movement/look are per-frame (already
/// smooth), so there are no prev/current pairs to interpolate.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BodyState {
    /// Body facing yaw in radians (engine yaw space).
    pub body_yaw: f32,
    /// Head yaw relative to the body, and look pitch (radians).
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// Normalized stride phase; each clip supplies its own duration.
    pub anim_time: f32,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle), eased by the
    /// game so starts/stops transition instead of snapping.
    pub walk_weight: f32,
    /// Sneak-stance blend weight (`0` upright … `1` crouched), eased like the
    /// walk blend. Cross-fades the authored `sneak` clip in: frame 0 while
    /// standing, its own cycle (instead of `walk`) while moving.
    pub sneak_weight: f32,
    pub locomotion: LocomotionBlend,
    /// Asleep in a bed: lying on the back, feet at the body's position, head
    /// toward `body_yaw`; head-look and the arm swing are suppressed.
    pub sleeping: bool,
    /// Seated on a mob seat or a sitting anchor: thighs swing forward and
    /// shins hang from the knees, walk/sneak layers rest; head-look and the
    /// arm swing stay live so a rider can look and punch.
    pub seated: bool,
    /// The mount's body tilt: a seated body leans with its mount about the
    /// hips, so a rider sits IN a cart on a slope instead of upright through
    /// its front. Ignored unless `seated`.
    pub seat_tilt: Tilt,
    /// Hurt-flash intensity `[0, 1]`; a rise is a fresh hit.
    pub hurt: f32,
}

/// One rig-bone offset, resolved to a bone INDEX by the caller.
///
/// Composed onto whatever the body's own animation already put that bone at,
/// about the bone's posed pivot, and carried through every descendant bone —
/// so an offset on a shoulder moves the whole arm and its held item.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BoneOffset {
    /// Index into the player model's bone list.
    pub bone: usize,
    /// Rotation in DEGREES about the bone's pivot, applied X, Y, Z.
    pub rotation: [f32; 3],
    /// Translation in 1/16-BLOCK pixels, in the bone's frame.
    pub translation: [f32; 3],
    /// Whether this layers over the body's animation or REPLACES that bone's
    /// share of it (a stance, which must not also swing with the stride).
    pub hold: bool,
}

/// What the local player's body is doing this frame, as the first-person
/// animator's driver reads it beside the two hands' frames.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LocalMotion {
    /// Horizontal speed, blocks per second.
    pub speed: f32,
    /// Horizontal velocity along the look's forward and rightward axes.
    pub forward: f32,
    pub strafe: f32,
    /// Vertical velocity, blocks per second, up positive.
    pub vertical: f32,
    pub grounded: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    pub swimming: bool,
    pub climbing: bool,
    /// Look pitch in degrees, up positive.
    pub pitch: f32,
    /// How fast the look is turning, degrees per second (rightward, upward).
    pub yaw_rate: f32,
    pub pitch_rate: f32,
    /// The walk bob: 0..1 through a stride, and how much of it is playing.
    pub stride: f32,
    pub stride_weight: f32,
    /// Seconds of hurt shake left; a rise is a fresh hit.
    pub hurt: f32,
    /// What the crosshair rests on within reach.
    pub target: AimTarget,
}

/// What the local player's crosshair rests on within reach.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AimTarget {
    #[default]
    Nothing = 0,
    Block = 1,
    Creature = 2,
}
