//! The offline mixdown: the world lane mixed on rodio's own device-free
//! mixer and pulled as PCM on the caller's clock instead of a sound card's.
//!
//! Nothing here plays a sound. Every play path in the engine is unchanged —
//! it joins whichever mixer the world lane points at — so a mixdown carries
//! exactly the gain, pitch, distance and spatialisation the device would
//! have. rodio applies controls on sample counts, never on wall time, so
//! pulling faster or slower than real time is correct by construction.

use rodio::mixer::{self, Mixer, MixerSource};
use rodio::{ChannelCount, SampleRate, Source};

use super::{connect_gain_loop, Audio, Lane};

/// Where the world lane mixes.
pub(super) enum WorldOutput {
    Device,
    Offline(OfflineMix),
}

impl WorldOutput {
    pub(super) fn is_offline(&self) -> bool {
        matches!(self, WorldOutput::Offline(_))
    }
}

pub(super) struct OfflineMix {
    mixer: Mixer,
    source: MixerSource,
    /// Frames owed but not yet pulled: the fraction a `dt` at this sample rate
    /// leaves over, carried so the track never drifts from the pictures.
    carry: f64,
}

impl OfflineMix {
    fn new(channels: u16, sample_rate: u32) -> Self {
        let channels = ChannelCount::new(channels.max(1)).unwrap_or(ChannelCount::MIN);
        let sample_rate = SampleRate::new(sample_rate.max(1)).unwrap_or(SampleRate::MIN);
        let (mixer, source) = mixer::mixer(channels, sample_rate);
        Self {
            mixer,
            source,
            carry: 0.0,
        }
    }

    pub(super) fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Append `frames` interleaved frames. The mixer answers `None` whenever
    /// nothing is sounding and recovers on the next call, so this is a counted
    /// loop: a gap is silence, never the end of the track.
    fn render(&mut self, frames: usize, out: &mut Vec<f32>) {
        let samples = frames * self.source.channels().get() as usize;
        out.reserve(samples);
        for _ in 0..samples {
            out.push(self.source.next().unwrap_or(0.0));
        }
    }
}

/// rodio re-reads a player's volume and speed every 5 ms of its own media
/// time (a spatial player's ear and emitter positions every 10 ms). Live, the
/// game writes new values at 150-250 fps, finer than that poll; an offline
/// frame is 16.7 ms at 60 fps, so a pull is cut into slices no longer than the
/// finer poll and the listener and emitters move a slice at a time. A fast
/// emitter then pans as smoothly as it does live instead of in frame steps.
const CONTROL_POLL_SECONDS: f64 = 0.005;
/// Bounds the slicing of one enormous pull (a stalled frame).
const MAX_SLICES: usize = 64;

impl Audio {
    /// A device-free engine whose world lane is an offline mixdown of
    /// `channels` at `sample_rate`, with pitch jitter and variant choice drawn
    /// from `seed` so the same session mixes down to the same samples.
    pub fn new_offline(channels: u16, sample_rate: u32, seed: u64) -> Self {
        let mut audio = Self::with_device(None, seed);
        audio.world = WorldOutput::Offline(OfflineMix::new(channels, sample_rate));
        audio
    }

    /// Move the world lane onto an offline mixdown (see
    /// [`new_offline`](Self::new_offline)). Every world sound already
    /// sounding carries over from the sample it had reached, and its sounds
    /// from here on mix there; the interface lane — menu clicks, the
    /// soundtrack — stays on the device. Beginning again restarts the
    /// mixdown, carrying the world's sounds over the same way.
    pub fn begin_offline(&mut self, channels: u16, sample_rate: u32, seed: u64) {
        self.rng = super::Xorshift::new(seed);
        self.world = WorldOutput::Offline(OfflineMix::new(channels, sample_rate));
        self.rebind_world_voices();
    }

    /// Hand the world lane back to the device, the sounds still sounding
    /// going on there from where the mixdown left them. Inert when no
    /// mixdown is open.
    pub fn end_offline(&mut self) {
        if self.world.is_offline() {
            self.world = WorldOutput::Device;
            self.rebind_world_voices();
        }
    }

    /// The open mixdown's `(channels, sample_rate)`, or `None` when the world
    /// lane is on the device.
    pub fn offline_format(&self) -> Option<(u16, u32)> {
        match &self.world {
            WorldOutput::Offline(mix) => {
                Some((mix.source.channels().get(), mix.source.sample_rate().get()))
            }
            WorldOutput::Device => None,
        }
    }

    /// Advance the mixdown by `dt` seconds, appending its interleaved `f32`
    /// frames to `out` and answering how many frames that was. Over any run
    /// of pulls the total stays within one frame of the summed `dt` times the
    /// sample rate. Call it once per presented frame, after
    /// [`update_spatial`](Self::update_spatial). Appends nothing when no
    /// mixdown is open.
    pub fn pull(&mut self, dt: f64, out: &mut Vec<f32>) -> usize {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let Some(mix) = self.offline_mut() else {
            return 0;
        };
        let owed = mix.carry + dt * mix.source.sample_rate().get() as f64;
        let frames = owed.floor() as usize;
        mix.carry = owed - frames as f64;
        let slices = ((dt / CONTROL_POLL_SECONDS).ceil() as usize).clamp(1, MAX_SLICES);
        let mut done = 0;
        for slice in 1..=slices {
            self.apply_spatial(slice as f32 / slices as f32);
            let end = frames * slice / slices;
            if let Some(mix) = self.offline_mut() {
                mix.render(end - done, out);
            }
            done = end;
        }
        frames
    }

    /// Move every world-lane voice onto the new output's mixer, each going
    /// on from the sample it had reached: the one-shots already sounding,
    /// the spatial sounds and the mod loops. They start again there in a
    /// fixed order, and none draws from the seeded stream, so a mixdown
    /// reproduces.
    fn rebind_world_voices(&mut self) {
        let Some(mixer) = self.mixer(Lane::World).cloned() else {
            self.clear_spatial();
            self.stop_gain_loops();
            self.oneshots.clear();
            return;
        };
        for old in std::mem::take(&mut self.oneshots) {
            let clip = &self.buffers[old.shot.sound.0 as usize][old.shot.variant];
            let from = old.cursor.frame(clip, false);
            old.sink.stop();
            if let Some(from) = from {
                self.sound_one_shot(old.shot, Lane::World, from);
            }
        }
        let volumes = self.lane_volumes(Lane::World);
        let mut handles: Vec<u64> = self.spatial.keys().copied().collect();
        handles.sort_unstable();
        for handle in handles {
            let Some(mut active) = self.spatial.remove(&handle) else {
                continue;
            };
            let def = active.sound.def();
            let clip = &self.buffers[active.sound.0 as usize][active.variant];
            let Some(from) = active.cursor.frame(clip, def.looped) else {
                active.sink.stop();
                continue;
            };
            let listener =
                self.listener_applied
                    .or(self.listener_target)
                    .unwrap_or(crate::SpatialListener {
                        pos: active.applied,
                        right: petramond_math::math::Vec3::X,
                    });
            let (player, cursor) = self.connect_spatial(
                &mixer,
                active.sound,
                active.variant,
                listener,
                active.applied,
                from,
            );
            player.set_volume(
                volumes.gain(def.category)
                    * active.local_gain
                    * active
                        .sound
                        .distance_gain((active.applied - listener.pos).length()),
            );
            player.set_speed(active.pitch);
            std::mem::replace(&mut active.sink, player).stop();
            active.cursor = cursor;
            self.spatial.insert(handle, active);
        }
        let mut sounds: Vec<crate::Sound> = self.gain_loops.keys().copied().collect();
        sounds.sort_unstable_by_key(|sound| sound.0);
        for sound in sounds {
            let Some(buf) = self
                .buffers
                .get(sound.0 as usize)
                .and_then(|variants| variants.first())
            else {
                continue;
            };
            let volume = volumes.gain(sound.def().category) * sound.def().gain;
            if let Some(active) = self.gain_loops.get_mut(&sound) {
                let from = active.cursor.frame(buf, true).unwrap_or(0);
                let (player, cursor) = connect_gain_loop(&mixer, buf, volume * active.gain, from);
                std::mem::replace(&mut active.sink, player).stop();
                active.cursor = cursor;
            }
        }
    }

    fn offline_mut(&mut self) -> Option<&mut OfflineMix> {
        match &mut self.world {
            WorldOutput::Offline(mix) => Some(mix),
            WorldOutput::Device => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::DecodedSound;
    use super::*;
    use crate::{Sound, SpatialListener, SpatialSoundSource};
    use petramond_math::math::Vec3;
    use petramond_math::world_pos::WorldPos;

    const RATE: u32 = 48_000;

    /// Replace `sound`'s clips with one steady mono tone, so a test reads the
    /// mixer's behaviour rather than an authored clip's envelope.
    fn steady(audio: &mut Audio, sound: Sound, seconds: f32) {
        audio.buffers[sound.0 as usize] = vec![DecodedSound {
            channels: ChannelCount::new(1).unwrap(),
            sample_rate: SampleRate::new(RATE).unwrap(),
            samples: vec![0.5; (seconds * RATE as f32) as usize].into(),
        }];
    }

    #[test]
    fn a_pull_is_exactly_its_frames_and_a_silent_gap_is_not_the_end() {
        let mut audio = Audio::new_offline(2, RATE, 7);
        steady(&mut audio, Sound::WoodPunch, 0.002);
        audio.play(Sound::WoodPunch);
        let mut out = Vec::new();
        // The clip ends inside the first pull; every pull after it runs over
        // an empty mixer.
        for _ in 0..10 {
            let before = out.len();
            assert_eq!(audio.pull(0.01, &mut out), 480);
            assert_eq!(out.len() - before, 480 * 2);
        }
        assert!(out[..480].iter().any(|s| *s != 0.0));
        assert!(out[out.len() - 480..].iter().all(|s| *s == 0.0));

        audio.play(Sound::WoodPunch);
        out.clear();
        audio.pull(0.01, &mut out);
        assert!(
            out.iter().any(|s| *s != 0.0),
            "a sound played after a gap still reaches the track"
        );
    }

    #[test]
    fn a_played_clip_mixes_down_to_sound() {
        let mut audio = Audio::new_offline(2, RATE, 1);
        audio.play(Sound::WoodPunch);
        let mut out = Vec::new();
        audio.pull(0.25, &mut out);
        assert!(out.iter().any(|s| s.abs() > 1e-4));
    }

    #[test]
    fn fractional_frames_carry_so_the_track_never_drifts() {
        let mut audio = Audio::new_offline(2, RATE, 1);
        let dt = 1.0 / 59.94;
        let mut out = Vec::new();
        let mut frames = 0;
        for _ in 0..1000 {
            frames += audio.pull(dt, &mut out);
        }
        let expected = 1000.0 * dt * RATE as f64;
        assert!(
            (frames as f64 - expected).abs() <= 1.0,
            "{frames} frames for {expected}"
        );
        assert_eq!(out.len(), frames * 2);
    }

    fn mixdown(seed: u64, interface_clicks: bool) -> Vec<f32> {
        let mut audio = Audio::new_offline(2, RATE, seed);
        let listener = SpatialListener {
            pos: WorldPos::new(0.5, 64.0, 0.5),
            right: Vec3::X,
        };
        let mut out = Vec::new();
        for frame in 0..30u64 {
            if interface_clicks {
                audio.play_interface(Sound::WoodPunch);
            }
            if frame % 5 == 0 {
                audio.play(Sound::WoodPunch);
                audio.play_spatial_randomized(
                    frame + 1,
                    Sound::WoodPunch,
                    SpatialSoundSource::Fixed(WorldPos::new(3.0, 64.0, 0.5)),
                    listener,
                    WorldPos::new(3.0, 64.0, 0.5),
                );
            }
            audio.update_spatial(listener, &[]);
            audio.pull(1.0 / 60.0, &mut out);
        }
        out
    }

    #[test]
    fn the_same_seed_mixes_down_to_the_same_samples() {
        let first = mixdown(0x5eed, false);
        assert!(first.iter().any(|s| *s != 0.0));
        assert!(
            first == mixdown(0x5eed, false),
            "the mixdown is reproducible"
        );
        assert!(
            first == mixdown(0x5eed, true),
            "interface plays never shift the world lane's sequence"
        );
    }

    #[test]
    fn a_mixdown_ignores_the_viewers_sliders() {
        let pull = |sliders: bool| {
            let mut audio = Audio::new_offline(2, RATE, 1);
            if sliders {
                audio.set_volumes(0.0, 0.0, 0.0);
            }
            audio.play(Sound::WoodPunch);
            let mut out = Vec::new();
            audio.pull(0.25, &mut out);
            out
        };
        assert!(pull(true) == pull(false));
    }

    #[test]
    fn a_world_loop_carries_onto_a_new_mixdown() {
        let mut audio = Audio::new_offline(2, RATE, 5);
        steady(&mut audio, Sound::WoodPunch, 0.1);
        audio.update_gain_loops(&[(Sound::WoodPunch, 1.0)], 0.25);
        audio.begin_offline(2, RATE, 5);
        let mut out = Vec::new();
        audio.pull(0.05, &mut out);
        assert!(out.iter().any(|s| *s != 0.0));
    }

    /// Replace `sound`'s clips with one mono rising ramp, so a sample's value
    /// says where in the clip it was read.
    fn ramp(audio: &mut Audio, sound: Sound, seconds: f32) {
        let frames = (seconds * RATE as f32) as usize;
        audio.buffers[sound.0 as usize] = vec![DecodedSound {
            channels: ChannelCount::new(1).unwrap(),
            sample_rate: SampleRate::new(RATE).unwrap(),
            samples: (0..frames)
                .map(|i| (i + 1) as f32 / frames as f32)
                .collect(),
        }];
    }

    #[test]
    fn sounds_already_sounding_go_on_in_the_next_mixdown_from_where_they_were() {
        let mut audio = Audio::new_offline(2, RATE, 11);
        ramp(&mut audio, Sound::WoodPunch, 0.5);
        let listener = SpatialListener {
            pos: WorldPos::new(0.5, 64.0, 0.5),
            right: Vec3::X,
        };
        let at = WorldPos::new(0.5, 64.0, 2.5);
        let play_both = |audio: &mut Audio| {
            audio.play(Sound::WoodPunch);
            audio.play_spatial(
                1,
                Sound::WoodPunch,
                SpatialSoundSource::Fixed(at),
                1.0,
                1.0,
                listener,
                at,
            );
            audio.update_spatial(listener, &[]);
        };
        // The same two sounds, never interrupted: the reference.
        play_both(&mut audio);
        let mut whole = Vec::new();
        audio.pull(0.2, &mut whole);
        audio.pull(0.2, &mut whole);

        let mut audio = Audio::new_offline(2, RATE, 11);
        ramp(&mut audio, Sound::WoodPunch, 0.5);
        play_both(&mut audio);
        let mut before = Vec::new();
        audio.pull(0.2, &mut before);
        audio.begin_offline(2, RATE, 11);
        let mut after = Vec::new();
        audio.pull(0.2, &mut after);

        // rodio's resampling reads a little ahead of what it has played, so
        // the seam may slip by a millisecond or so; a sound that
        // restarted or went missing is off by a third of the ramp.
        let slip = 2.0 * 0.002 * 1.5 / 0.5;
        let seam = before.len();
        let worst = whole[seam..]
            .iter()
            .zip(&after)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < slip,
            "the new mixdown goes on where the old one was (worst {worst})"
        );
    }

    #[test]
    fn a_moving_emitter_pans_within_one_frame() {
        let mut audio = Audio::new_offline(2, RATE, 3);
        steady(&mut audio, Sound::WoodPunch, 1.0);
        let listener = SpatialListener {
            pos: WorldPos::new(0.5, 64.0, 0.5),
            right: Vec3::X,
        };
        let left = WorldPos::new(-1.5, 64.0, 0.5);
        let right = WorldPos::new(2.5, 64.0, 0.5);
        audio.play_spatial(
            1,
            Sound::WoodPunch,
            SpatialSoundSource::Mob(9),
            1.0,
            1.0,
            listener,
            left,
        );
        let mut out = Vec::new();
        audio.update_spatial(listener, &[(9, left)]);
        audio.pull(1.0 / 30.0, &mut out);

        audio.update_spatial(listener, &[(9, right)]);
        out.clear();
        audio.pull(1.0 / 30.0, &mut out);
        let energy = |frames: &[f32]| {
            frames
                .chunks(2)
                .fold((0.0, 0.0), |(l, r), f| (l + f[0].abs(), r + f[1].abs()))
        };
        let window = 240 * 2;
        let (head_l, head_r) = energy(&out[..window]);
        let (tail_l, tail_r) = energy(&out[out.len() - window..]);
        assert!(
            head_l > head_r && tail_r > tail_l,
            "the frame opens panned to the listener's left, where the emitter was, \
             and closes on their right, where it went: \
             head {head_l}/{head_r}, tail {tail_l}/{tail_r}"
        );
    }
}
