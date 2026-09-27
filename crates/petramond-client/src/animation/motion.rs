use petramond_math::math::Tilt;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SwimBlend {
    pub grounded: f32,
    pub weight: f32,
    pub phase: f32,
    pub moving: f32,
    pub backward: f32,
    pub rising: f32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LocomotionBlend {
    pub swim: SwimBlend,
    pub run: f32,
    pub backward: f32,
    pub airborne: f32,
    pub falling: f32,
    pub landing: f32,
    pub strafe: f32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BodyState {
    pub body_yaw: f32,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub anim_time: f32,
    pub walk_weight: f32,
    pub sneak_weight: f32,
    pub locomotion: LocomotionBlend,
    pub sleeping: bool,
    pub seated: bool,
    pub seat_tilt: Tilt,
    pub hurt: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BoneOffset {
    pub bone: usize,
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub hold: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LocalMotion {
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
    pub target: AimTarget,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AimTarget {
    #[default]
    Nothing = 0,
    Block = 1,
    Creature = 2,
}
