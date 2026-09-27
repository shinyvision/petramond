pub mod music_registry;

#[cfg_attr(not(feature = "playback"), allow(unused_imports))]
pub use music_registry::MusicTrack;
#[cfg_attr(not(feature = "playback"), allow(unused_imports))]
pub use petramond_world::sound_registry::{Sound, SoundCategory};

#[cfg(feature = "playback")]
mod keep_alive;

#[cfg(feature = "playback")]
mod engine;
#[cfg(not(feature = "playback"))]
#[path = "engine_off.rs"]
mod engine;

pub use engine::Audio;

pub mod convert;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SpatialListener {
    pub pos: petramond_math::world_pos::WorldPos,
    pub right: petramond_math::math::Vec3,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SpatialSoundSource {
    Fixed(petramond_math::world_pos::WorldPos),
    Mob(u64),
}
