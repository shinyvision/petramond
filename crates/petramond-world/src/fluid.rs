//! Shared fluid properties, immersion and forces for body controllers.

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

/// A body's response to immersion, independent of the fluid it enters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Buoyancy {
    /// Stroke upward on swim intent and sink gently when passive.
    #[default]
    Swim,
    /// Settle at the fluid surface without bobbing.
    Surface,
    /// Cancel gravity while retaining motion subject to fluid resistance.
    Neutral,
}

/// How a swimmer clears a reachable shore lip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FluidClimb {
    Jump,
    Step,
}

/// Fluid-owned resistance and buoyancy; body controllers supply movement intent.
/// Speeds are m/s, acceleration m/s², and friction is the fraction shed per 60 Hz frame.
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

/// Current strength; zero speed means this fluid does not carry bodies.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentProperties {
    pub speed: f32,
    pub accel: f32,
}

/// Directional cooling: contact with `by` turns the receiving cell into `result`.
#[derive(Clone, Copy, Debug)]
pub struct Quench {
    pub by: Block,
    pub result: Block,
}

/// What a body falling into a fluid throws up: a one-shot emitter burst at the
/// surface and a sound whose tier follows the fall height.
#[derive(Clone, Copy, Debug)]
pub struct FluidSplash {
    /// A `particle_emitters.json` burst bundle id.
    pub burst: u8,
    pub sound_small: crate::sound_registry::Sound,
    pub sound_big: crate::sound_registry::Sound,
}

/// A fluid block's resolved row. Simulation and body physics read the same
/// descriptor, always by `&'static` reference to the loaded table.
#[derive(Debug)]
pub struct FluidDef {
    pub block: Block,
    /// The block row's registry name.
    pub name: &'static str,
    pub delay: u64,
    pub drop_off: u8,
    pub renewable: bool,
    pub quench: Option<Quench>,
    pub motion: FluidMotion,
    pub current: CurrentProperties,
    /// What a falling body throws up on entry; `None` enters silently.
    pub splash: Option<FluidSplash>,
    pub contact: FluidContact,
    pub medium: FluidMedium,
}

/// A body's sampled fluid and the absolute height of that fluid's surface.
#[derive(Clone, Copy, Debug)]
pub struct Immersion {
    pub fluid: &'static FluidDef,
    pub surface_y: f32,
}

/// Current at a point, after resolving the fluid's direction and strength.
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
