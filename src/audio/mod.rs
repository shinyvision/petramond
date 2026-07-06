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

mod keep_alive;
mod registry;

pub use registry::{Sound, SoundCategory};

pub(crate) use registry::by_name as sound_by_name;

use std::collections::HashMap;
use std::io::Cursor;

use rodio::buffer::SamplesBuffer;
use rodio::source::Source;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, SampleRate, SpatialPlayer};

use keep_alive::KeepAlive;
use registry::defs as sound_defs;

/// Mining punch sounds retrigger at a fixed cadence while held. Each trigger is a
/// one-shot mixed over any previous trigger, so long clips can overlap naturally.
const MINING_REPEAT_INTERVAL: f64 = 0.300;
const EAR_HALF_SPACING: f32 = 0.18;

/// A sound decoded into memory once at startup, replayed by cloning the sample
/// buffer (a memcpy — far cheaper than re-decoding the OGG on every play).
struct DecodedSound {
    channels: ChannelCount,
    sample_rate: SampleRate,
    samples: Vec<f32>,
}

impl DecodedSound {
    /// Playback length at unit speed, in seconds — read from the decoded clip itself
    /// (frames ÷ sample rate), so decode checks do not pin asset metadata.
    #[inline]
    #[cfg(test)]
    fn duration(&self) -> f64 {
        let frames = self.samples.len() / self.channels.get() as usize;
        frames as f64 / self.sample_rate.get() as f64
    }
}

/// Listener state for active spatial sounds, derived by the app from the
/// current camera every frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SpatialListener {
    pub(crate) pos: crate::mathh::Vec3,
    pub(crate) right: crate::mathh::Vec3,
}

impl SpatialListener {
    fn audio_space(
        self,
        emitter: crate::mathh::Vec3,
        attenuation_distance: f32,
    ) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let scale = attenuation_distance.max(1.0);
        let emitter = (emitter - self.pos) / scale;
        let right = self.right.normalize_or_zero() * EAR_HALF_SPACING;
        (
            vec3(emitter),
            vec3(crate::mathh::Vec3::ZERO - right),
            vec3(crate::mathh::Vec3::ZERO + right),
        )
    }
}

/// Where an active spatial sound gets its emitter position.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum SpatialSoundSource {
    Fixed(crate::mathh::Vec3),
    Mob(u64),
}

struct ActiveSpatialSound {
    sink: SpatialPlayer,
    sound: Sound,
    source: SpatialSoundSource,
    base_gain: f32,
    pitch: f32,
    last_position: crate::mathh::Vec3,
}

/// The audio engine: owns the output stream and the decoded sound buffers, and
/// drives at most one repeated sound (e.g. mining). Lives on the client (the `App`);
/// the simulation never touches it.
pub struct Audio {
    /// OS output stream + mixer. `None` when no device opened (runs silent). Kept
    /// alive for the lifetime of `Audio`: dropping it stops all playback.
    sink: Option<MixerDeviceSink>,
    /// Decoded variant buffers per sound, indexed by raw sound id (parallel to
    /// the loaded sound table). Each sound holds a list of interchangeable clips; a play
    /// picks one at random. An empty list (all variants failed to decode) is silent.
    buffers: Vec<Vec<DecodedSound>>,
    /// Master linear gain over every sound (a future global volume control).
    master_gain: f32,
    /// xorshift64 state for per-play pitch jitter. Presentation-only randomness,
    /// seeded from the wall clock so runs differ; only the sequence matters.
    rng: u64,
    #[cfg(test)]
    played: Vec<Sound>,
    /// The sound currently repeating (e.g. mining) and the wall-clock time its next
    /// trigger is due. Driven per-frame by [`set_loop`](Self::set_loop).
    loop_sound: Option<Sound>,
    loop_next: f64,
    spatial: HashMap<u64, ActiveSpatialSound>,
}

impl Audio {
    /// Open the default audio device and decode every sound. Best-effort: failing to
    /// open the device (headless / no speaker) or to decode a sound logs a warning
    /// and leaves that part silent — never an error.
    pub fn new() -> Self {
        let sink = match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                // Keep the OS audio device — and especially a Bluetooth link — awake
                // for the whole session by mixing in a continuous inaudible signal, so
                // the first sound after a silent gap isn't delayed by a 200-500 ms
                // device cold-start (see `keep_alive`).
                let cfg = sink.config();
                sink.mixer()
                    .add(KeepAlive::new(cfg.channel_count(), cfg.sample_rate()));
                Some(sink)
            }
            Err(e) => {
                log::warn!("audio disabled: could not open output device: {e}");
                None
            }
        };
        let buffers = sound_defs()
            .iter()
            .map(|def| {
                def.variants
                    .iter()
                    .filter_map(|&rel| {
                        let Some((bytes, _)) = crate::assets::read_bytes(rel) else {
                            log::warn!("sound {:?} clip '{rel}' not found (skipped)", def.sound);
                            return None;
                        };
                        match decode(bytes) {
                            Ok(d) => Some(d),
                            Err(e) => {
                                log::warn!(
                                    "sound {:?} variant failed to decode (skipped): {e}",
                                    def.sound
                                );
                                None
                            }
                        }
                    })
                    .collect()
            })
            .collect();
        Self {
            sink,
            buffers,
            master_gain: 1.0,
            rng: seed_rng(),
            #[cfg(test)]
            played: Vec::new(),
            loop_sound: None,
            loop_next: 0.0,
            spatial: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn take_played_for_test(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.played)
    }

    /// Drive a repeating sound (e.g. the mining "punch"). Call every frame with the
    /// sound that should be repeating right now (`None` = stop) and the current
    /// wall-clock time `now`.
    ///
    /// It starts the instant the sound changes, then triggers a fresh randomized
    /// one-shot every 300 ms while active. Each trigger is mixed in rather than
    /// replacing the previous one, so in-flight plays finish naturally and can layer.
    pub fn set_loop(&mut self, sound: Option<Sound>, now: f64) {
        match sound {
            None => self.loop_sound = None,
            Some(s) => {
                let changed = self.loop_sound != Some(s);
                if changed || now >= self.loop_next {
                    self.emit(s, 1.0);
                    self.loop_sound = Some(s);
                    self.loop_next = now + MINING_REPEAT_INTERVAL;
                }
            }
        }
    }

    /// Play a one-shot sound (e.g. a block being placed): a random variant at a random
    /// pitch, fire-and-forget. No-op if audio is disabled or the sound didn't decode.
    pub fn play(&mut self, sound: Sound) {
        self.emit(sound, 1.0);
    }

    /// [`play`](Self::play) with an extra linear gain factor on top of the
    /// sound's own — the distance-attenuation hook for positional (mod-emitted)
    /// sounds. A non-positive gain skips the play entirely.
    pub fn play_attenuated(&mut self, sound: Sound, gain: f32) {
        if gain > 0.0 {
            self.emit(sound, gain);
        }
    }

    /// Start or replace an active spatial sound. No-op when audio is disabled,
    /// the sound has no decoded variants, or the handle is zero.
    pub(crate) fn play_spatial(
        &mut self,
        handle: u64,
        sound: Sound,
        source: SpatialSoundSource,
        volume: f32,
        pitch: f32,
        listener: SpatialListener,
        initial_position: crate::mathh::Vec3,
    ) {
        if handle == 0 || volume <= 0.0 || pitch <= 0.0 {
            return;
        }
        let count = self.buffers.get(sound.0 as usize).map_or(0, Vec::len);
        if count == 0 {
            return;
        }
        let variant = self.next_index(count);
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        let def = sound.def();
        let base_gain = self.master_gain * category_gain(def.category) * def.gain * volume;
        let buf = &self.buffers[sound.0 as usize][variant];
        let (emitter, left_ear, right_ear) =
            listener.audio_space(initial_position, def.attenuation_distance);
        let player = SpatialPlayer::connect_new(sink.mixer(), emitter, left_ear, right_ear);
        player.set_volume(
            base_gain * sound.distance_gain((initial_position - listener.pos).length()),
        );
        player.set_speed(pitch);
        player.append(SamplesBuffer::new(
            buf.channels,
            buf.sample_rate,
            buf.samples.clone(),
        ));
        self.spatial.insert(
            handle,
            ActiveSpatialSound {
                sink: player,
                sound,
                source,
                base_gain,
                pitch,
                last_position: initial_position,
            },
        );
    }

    /// Start a presentation-owned one-shot spatial sound using the row's own
    /// gain and pitch jitter. This is for engine presentation events such as
    /// mob hurt/death calls; deterministic mod HostCalls keep using
    /// [`play_spatial`](Self::play_spatial), where the guest supplies pitch.
    pub(crate) fn play_spatial_randomized(
        &mut self,
        handle: u64,
        sound: Sound,
        source: SpatialSoundSource,
        listener: SpatialListener,
        initial_position: crate::mathh::Vec3,
    ) {
        let pitch_variation = sound.def().pitch_variation;
        let pitch = 1.0 + self.next_jitter() * pitch_variation;
        self.play_spatial(
            handle,
            sound,
            source,
            1.0,
            pitch,
            listener,
            initial_position,
        );
    }

    /// Stop a mod-owned spatial sound. Unknown handles are intentionally inert.
    pub(crate) fn stop_spatial(&mut self, handle: u64) {
        if let Some(active) = self.spatial.remove(&handle) {
            active.sink.stop();
        }
    }

    pub(crate) fn clear_spatial(&mut self) {
        for (_, active) in self.spatial.drain() {
            active.sink.stop();
        }
    }

    /// Refresh active spatial sounds from the current camera and the same
    /// per-frame mob positions the renderer consumes. A mob-pinned sound whose
    /// mob id is absent keeps its last position and is allowed to finish there.
    pub(crate) fn update_spatial(
        &mut self,
        listener: SpatialListener,
        mobs: &[(u64, crate::mathh::Vec3)],
    ) {
        if self.sink.is_none() {
            self.spatial.clear();
            return;
        }
        for active in self.spatial.values_mut() {
            let pos = match active.source {
                SpatialSoundSource::Fixed(pos) => pos,
                SpatialSoundSource::Mob(id) => mobs
                    .iter()
                    .find(|(mob_id, _)| *mob_id == id)
                    .map(|(_, pos)| *pos)
                    .unwrap_or(active.last_position),
            };
            active.last_position = pos;
            let attenuation_distance = active.sound.def().attenuation_distance;
            let (emitter, left_ear, right_ear) = listener.audio_space(pos, attenuation_distance);
            active.sink.set_emitter_position(emitter);
            active.sink.set_left_ear_position(left_ear);
            active.sink.set_right_ear_position(right_ear);
            active.sink.set_volume(
                active.base_gain * active.sound.distance_gain((pos - listener.pos).length()),
            );
            active.sink.set_speed(active.pitch);
        }
        self.spatial.retain(|_, active| !active.sink.empty());
    }

    /// Play `sound` once with fresh random pitch and its gain (scaled by
    /// `extra_gain`). No-op if audio is disabled or the sound didn't decode.
    fn emit(&mut self, sound: Sound, extra_gain: f32) {
        let def = sound.def();
        // How many decoded variants this sound has (immutable borrow, released here).
        let count = self.buffers.get(sound.0 as usize).map_or(0, Vec::len);
        if count == 0 {
            return;
        }
        #[cfg(test)]
        self.played.push(sound);
        // All randomness up front — the only `&mut self` use, so the buffer borrow
        // below doesn't conflict: a random variant (so a repeated sound isn't the
        // same clip) plus the per-play pitch jitter.
        let variant = self.next_index(count);
        let pitch = 1.0 + self.next_jitter() * def.pitch_variation;
        let gain = self.master_gain * category_gain(def.category) * def.gain * extra_gain;

        let buf = &self.buffers[sound.0 as usize][variant];
        if let Some(sink) = self.sink.as_ref() {
            // Clone the decoded PCM into a fresh replayable source, shift its pitch
            // and gain, and mix it in. `speed` resamples (pitch + tempo together);
            // `add` overlaps it with anything already playing.
            let source = SamplesBuffer::new(buf.channels, buf.sample_rate, buf.samples.clone())
                .speed(pitch)
                .amplify(gain);
            sink.mixer().add(source);
        }
    }

    /// Step the xorshift64 stream. Presentation-only randomness (variant + pitch),
    /// never the deterministic worldgen RNG.
    #[inline]
    fn next_rng(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// Next pitch jitter in `[-1.0, 1.0)`.
    fn next_jitter(&mut self) -> f32 {
        // Top 24 bits → [0, 1) → [-1, 1).
        let unit = (self.next_rng() >> 40) as f32 / (1u32 << 24) as f32;
        unit * 2.0 - 1.0
    }

    /// A uniform-ish random index in `[0, len)` (returns 0 when `len <= 1`). The
    /// modulo bias is negligible for the handful of variants a sound has.
    fn next_index(&mut self, len: usize) -> usize {
        if len <= 1 {
            return 0;
        }
        (self.next_rng() % len as u64) as usize
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-category gain multiplier. Full volume for every category today; the hook for
/// a future per-category (block / UI) volume control.
fn category_gain(category: SoundCategory) -> f32 {
    match category {
        SoundCategory::Block | SoundCategory::Mob | SoundCategory::Ui => 1.0,
    }
}

fn vec3(v: crate::mathh::Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// Decode OGG/Vorbis `bytes` into an in-memory PCM buffer (f32 samples + format).
/// Device-free and split out so it is unit-testable without an audio device.
fn decode(bytes: Vec<u8>) -> Result<DecodedSound, String> {
    let decoder =
        rodio::Decoder::try_from(Cursor::new(bytes)).map_err(|e| format!("decode init: {e}"))?;
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    let samples: Vec<f32> = decoder.collect();
    if samples.is_empty() {
        return Err("decoded to zero samples".into());
    }
    Ok(DecodedSound {
        channels,
        sample_rate,
        samples,
    })
}

/// A non-deterministic, presentation-only seed for the pitch-jitter RNG, from the
/// wall clock so different runs vary. The fixed fallback keeps it infallible; the
/// `| 1` guarantees the non-zero state xorshift64 requires.
fn seed_rng() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        | 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device-less engine for exercising the pure RNG helpers.
    fn silent_audio(seed: u64) -> Audio {
        Audio {
            sink: None,
            buffers: Vec::new(),
            master_gain: 1.0,
            rng: seed,
            played: Vec::new(),
            loop_sound: None,
            loop_next: 0.0,
            spatial: HashMap::new(),
        }
    }

    #[test]
    fn wood_punch_variant_decodes_to_pcm() {
        // A variant decodes to real PCM with no audio device involved — proving the
        // embed + decode path. Format isn't pinned (freely-edited asset data): we
        // only require a sane, playable buffer with a real duration.
        let rel = sound_defs()[Sound::WoodPunch.0 as usize].variants[0];
        let bytes = crate::assets::read_bytes(rel).expect("clip file exists").0;
        let d = decode(bytes).expect("wood_punch variant should decode");
        assert!(!d.samples.is_empty(), "decoded to some samples");
        assert!(d.sample_rate.get() > 0);
        assert!(d.channels.get() >= 1);
        assert!(d.duration() > 0.0, "has a positive duration");
    }

    #[test]
    fn every_sound_has_at_least_one_variant() {
        // A sound with no clips would be silently silent — a data mistake, not a
        // tunable value, so this guards the structure without pinning the count.
        for def in sound_defs() {
            assert!(!def.variants.is_empty(), "{:?} has no clips", def.sound);
        }
    }

    #[test]
    fn jitter_stays_in_unit_range() {
        let mut a = silent_audio(0x1234_5678_9abc_def1);
        for _ in 0..10_000 {
            let j = a.next_jitter();
            assert!((-1.0..1.0).contains(&j), "jitter {j} out of range");
        }
    }

    #[test]
    fn random_variant_stays_in_range_and_covers_all() {
        let mut a = silent_audio(0xDEAD_BEEF_CAFE_1234);
        let mut seen = [false; 3];
        for _ in 0..1_000 {
            let i = a.next_index(3);
            assert!(i < 3, "index {i} out of range");
            seen[i] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "every variant should be chosen over many plays"
        );
        // Degenerate counts never panic or index out of bounds.
        assert_eq!(a.next_index(1), 0);
        assert_eq!(a.next_index(0), 0);
    }

    #[test]
    fn mining_loop_retriggers_on_fixed_cadence() {
        let mut a = silent_audio(0x1234_5678_9abc_def1);

        a.set_loop(Some(Sound::WoodPunch), 10.0);
        assert_eq!(a.loop_sound, Some(Sound::WoodPunch));
        assert_close(a.loop_next, 10.0 + MINING_REPEAT_INTERVAL);

        let first_next = a.loop_next;
        a.set_loop(Some(Sound::WoodPunch), first_next - 0.001);
        assert_close(a.loop_next, first_next);

        a.set_loop(Some(Sound::WoodPunch), first_next);
        assert_close(a.loop_next, first_next + MINING_REPEAT_INTERVAL);
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1e-9,
            "expected {expected}, got {actual}"
        );
    }
}
