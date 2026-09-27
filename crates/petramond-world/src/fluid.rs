use crate::block::Block;
use crate::mathh::Vec3;

pub mod contact;
pub(crate) mod load;
pub mod medium;
mod motion;
mod query;
pub use contact::{ConditionGrant, FluidContact};
pub use medium::{AlbedoMix, FluidMedium};
pub use query::sample_body_current;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Buoyancy {
    #[default]
    Swim,
    Surface,
    Neutral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FluidClimb {
    Jump,
    Step,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FluidMotion {
    pub speed_scale: f32,
    pub accel: f32,
    pub friction: f32,
    pub rise: f32,
    pub sink: f32,
    pub vertical_accel: f32,
    pub entry_friction: f32,
    pub probe_fraction: f32,
    pub probe_offset: f32,
    pub climb: FluidClimb,
}

#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentProperties {
    pub speed: f32,
    pub accel: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Quench {
    pub by: Block,
    pub result: Block,
}

#[derive(Clone, Copy, Debug)]
pub struct FluidSplash {
    pub burst: u8,
    pub sound_small: crate::sound_registry::Sound,
    pub sound_big: crate::sound_registry::Sound,
}

#[derive(Debug)]
pub struct FluidDef {
    pub block: Block,
    pub name: &'static str,
    pub delay: u64,
    pub drop_off: u8,
    pub renewable: bool,
    pub quench: Option<Quench>,
    pub motion: FluidMotion,
    pub current: CurrentProperties,
    pub splash: Option<FluidSplash>,
    pub contact: FluidContact,
    pub medium: FluidMedium,
}

#[derive(Clone, Copy, Debug)]
pub struct Immersion {
    pub fluid: &'static FluidDef,
    pub surface_y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FluidCurrent {
    pub velocity: Vec3,
    pub accel: f32,
}

impl FluidCurrent {
    pub const NONE: Self = Self {
        velocity: Vec3::ZERO,
        accel: 0.0,
    };
}
