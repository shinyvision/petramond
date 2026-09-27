use super::{MusicTrack, Sound, SpatialListener, SpatialSoundSource};

pub struct Audio;

impl Audio {
    pub fn new() -> Self {
        Audio
    }

    pub fn new_offline(_channels: u16, _sample_rate: u32, _seed: u64) -> Self {
        Audio
    }

    pub fn begin_offline(&mut self, _channels: u16, _sample_rate: u32, _seed: u64) {}
    pub fn end_offline(&mut self) {}
    pub fn offline_format(&self) -> Option<(u16, u32)> {
        None
    }
    pub fn pull(&mut self, _dt: f64, _out: &mut Vec<f32>) -> usize {
        0
    }
    pub fn world_format(&self) -> Option<(u16, u32)> {
        None
    }
    pub fn mixes(&self) -> bool {
        false
    }
    pub fn has_device(&self) -> bool {
        false
    }
    pub fn set_world_copied(&mut self, _copied: bool) {}
    pub fn drain_world_copy(&mut self, _out: &mut Vec<f32>) {}

    #[cfg(any(test, feature = "test-support"))]
    pub fn take_played_for_test(&mut self) -> Vec<Sound> {
        Vec::new()
    }

    pub fn set_volumes(&mut self, _master: f32, _sound: f32, _music: f32) {}

    pub fn set_loop(&mut self, _sound: Option<Sound>, _now: f64) {}
    pub fn update_gain_loops(&mut self, _desired: &[(Sound, f32)], _dt: f32) {}
    pub fn stop_gain_loops(&mut self) {}

    pub fn play_music(&mut self, _track: MusicTrack) -> bool {
        false
    }
    pub fn stop_music(&mut self) {}
    pub fn music_playing(&self) -> Option<MusicTrack> {
        None
    }
    pub fn update_music(&mut self, _dt: f32) {}

    pub fn play(&mut self, _sound: Sound) {}
    pub fn play_interface(&mut self, _sound: Sound) {}

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
        _initial_position: petramond_math::world_pos::WorldPos,
    ) {
    }

    pub fn play_spatial_randomized(
        &mut self,
        _handle: u64,
        _sound: Sound,
        _source: SpatialSoundSource,
        _listener: SpatialListener,
        _initial_position: petramond_math::world_pos::WorldPos,
    ) {
    }

    pub fn set_spatial(&mut self, _handle: u64, _volume: f32, _pitch: f32) {}

    pub fn set_spatial_paused(&mut self, _paused: bool) {}

    pub fn stop_spatial(&mut self, _handle: u64) {}

    pub fn clear_spatial(&mut self) {}

    pub fn update_spatial(
        &mut self,
        _listener: SpatialListener,
        _mobs: &[(u64, petramond_math::world_pos::WorldPos)],
    ) {
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}
