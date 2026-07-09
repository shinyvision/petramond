//! Client-side sound playback.
//!
//! Audio is presentation, never simulation: it lives entirely on the client and is
//! NEVER driven from the game tick. The tick's job is in-game events; the client
//! observes game state each frame (e.g. "which block is being mined") and turns that
//! into sound here. The only non-deterministic ingredient — the per-play pitch
//! jitter — lives here by design.
//!
//! [`Audio`] is best-effort: if no output device opens, or a sound fails to decode,
//! it logs and runs silent rather than failing — a missing speaker never costs you
//! the game (mirroring [`crate::asset_cache`]'s never-fatal stance).
//!
//! The PLAYBACK half (rodio → cpal → ALSA on Linux) sits behind the default-on
//! `audio` cargo feature; without it [`Audio`] is a signature-identical silent
//! stub, so a headless server builds with no audio system libraries at all
//! (`cargo build --no-default-features --bin petramond_server`). The sound
//! REGISTRY (names, defs, categories — what the sim, block sounds, and the net
//! name tables consume) is data, not playback, and is always compiled.

mod registry;

// `SoundCategory` has no featureless consumer (only the engine's per-category
// gain reads it), but the re-export is part of the module's stable surface.
#[cfg_attr(not(feature = "audio"), allow(unused_imports))]
pub use registry::{Sound, SoundCategory};

pub(crate) use registry::by_name as sound_by_name;
pub(crate) use registry::defs as sound_defs_for_net;

#[cfg(feature = "audio")]
mod keep_alive;

#[cfg(feature = "audio")]
mod engine;
#[cfg(not(feature = "audio"))]
#[path = "engine_off.rs"]
mod engine;

pub use engine::Audio;

/// Listener state for active spatial sounds, derived by the app from the
/// current camera every frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SpatialListener {
    pub(crate) pos: crate::mathh::Vec3,
    pub(crate) right: crate::mathh::Vec3,
}

/// Where an active spatial sound gets its emitter position.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum SpatialSoundSource {
    Fixed(crate::mathh::Vec3),
    Mob(u64),
}
