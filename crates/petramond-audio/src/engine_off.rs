//! The featureless [`Audio`]: a silent stub with the playback engine's exact
//! surface, compiled when the `audio` cargo feature is OFF (headless server
//! builds — no rodio/cpal/ALSA at build or runtime). Every method is a no-op;
//! keep the signatures in lockstep with `audio/engine.rs` — drift shows up as
//! a compile error in `--no-default-features` builds (the Makefile's
//! `run-server` target is one).

use super::{Sound, SpatialListener, SpatialSoundSource};

/// The silent stand-in for the playback engine. See the module doc.
pub struct Audio;

impl Audio {
    pub fn new() -> Self {
        Audio
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn take_played_for_test(&mut self) -> Vec<Sound> {
        Vec::new()
    }

    pub fn set_volumes(&mut self, _master: f32, _sound: f32, _music: f32) {}

    pub fn set_loop(&mut self, _sound: Option<Sound>, _now: f64) {}
    pub fn update_gain_loops(&mut self, _desired: &[(Sound, f32)], _dt: f32) {}
    pub fn stop_gain_loops(&mut self) {}

    pub fn play(&mut self, _sound: Sound) {}

    pub fn play_attenuated(&mut self, _sound: Sound, _gain: f32) {}

    #[allow(clippy::too_many_arguments)]
    pub fn play_spatial(
        &mut self,
        _handle: u64,
        _sound: Sound,
        _source: SpatialSoundSource,
        _volume: f32,
        _pitch: f32,
        _listener: SpatialListener,
        _initial_position: petramond_math::math::Vec3,
    ) {
    }

    pub fn play_spatial_randomized(
        &mut self,
        _handle: u64,
        _sound: Sound,
        _source: SpatialSoundSource,
        _listener: SpatialListener,
        _initial_position: petramond_math::math::Vec3,
    ) {
    }

    pub fn set_spatial(&mut self, _handle: u64, _volume: f32, _pitch: f32) {}

    pub fn set_spatial_paused(&mut self, _paused: bool) {}

    pub fn stop_spatial(&mut self, _handle: u64) {}

    pub fn clear_spatial(&mut self) {}

    pub fn update_spatial(
        &mut self,
        _listener: SpatialListener,
        _mobs: &[(u64, petramond_math::math::Vec3)],
    ) {
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}
