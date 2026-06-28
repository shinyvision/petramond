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

use std::io::Cursor;

use rodio::buffer::SamplesBuffer;
use rodio::source::Source;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, SampleRate};

use keep_alive::KeepAlive;
use registry::SOUND_DEFS;

/// Floor on a looping sound's repeat period, so a sound with no decoded buffer
/// (duration `0`) can't retrigger every frame. Far below any real clip length.
const MIN_LOOP_PERIOD: f64 = 0.02;

/// A sound decoded into memory once at startup, replayed by cloning the sample
/// buffer (a memcpy — far cheaper than re-decoding the OGG on every play).
struct DecodedSound {
    channels: ChannelCount,
    sample_rate: SampleRate,
    samples: Vec<f32>,
}

impl DecodedSound {
    /// Playback length at unit speed, in seconds — read from the decoded clip itself
    /// (frames ÷ sample rate), so it tracks whatever asset is loaded with no
    /// hard-coded duration anywhere.
    #[inline]
    fn duration(&self) -> f64 {
        let frames = self.samples.len() / self.channels.get() as usize;
        frames as f64 / self.sample_rate.get() as f64
    }
}

/// The audio engine: owns the output stream and the decoded sound buffers, and
/// drives at most one looping sound (e.g. mining). Lives on the client (the `App`);
/// the simulation never touches it.
pub struct Audio {
    /// OS output stream + mixer. `None` when no device opened (runs silent). Kept
    /// alive for the lifetime of `Audio`: dropping it stops all playback.
    sink: Option<MixerDeviceSink>,
    /// Decoded variant buffers per sound, indexed by `sound as usize` (parallel to
    /// [`SOUND_DEFS`]). Each sound holds a list of interchangeable clips; a play
    /// picks one at random. An empty list (all variants failed to decode) is silent.
    buffers: Vec<Vec<DecodedSound>>,
    /// Master linear gain over every sound (a future global volume control).
    master_gain: f32,
    /// xorshift64 state for per-play pitch jitter. Presentation-only randomness,
    /// seeded from the wall clock so runs differ; only the sequence matters.
    rng: u64,
    /// The sound currently looping (e.g. mining) and the wall-clock time its next
    /// repeat is due. Driven per-frame by [`set_loop`](Self::set_loop).
    loop_sound: Option<Sound>,
    loop_next: f64,
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
        let buffers = SOUND_DEFS
            .iter()
            .map(|def| {
                def.variants
                    .iter()
                    .filter_map(|&bytes| match decode(bytes) {
                        Ok(d) => Some(d),
                        Err(e) => {
                            log::warn!(
                                "sound {:?} variant failed to decode (skipped): {e}",
                                def.sound
                            );
                            None
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
            loop_sound: None,
            loop_next: 0.0,
        }
    }

    /// Drive a looping sound (e.g. the mining "punch"). Call every frame with the
    /// sound that should be looping right now (`None` = stop) and the current
    /// wall-clock time `now`.
    ///
    /// It starts the instant the sound changes, then repeats exactly as each play
    /// finishes — the period is the clip's **own decoded length**, never a hard-coded
    /// interval, so it adapts to any asset — with fresh pitch each repeat so it never
    /// sounds robotic. Stopping just ends the repeats; the in-flight play finishes
    /// naturally.
    pub fn set_loop(&mut self, sound: Option<Sound>, now: f64) {
        match sound {
            None => self.loop_sound = None,
            Some(s) => {
                let changed = self.loop_sound != Some(s);
                if changed || now >= self.loop_next {
                    let played = self.emit(s);
                    self.loop_sound = Some(s);
                    self.loop_next = now + played.max(MIN_LOOP_PERIOD);
                }
            }
        }
    }

    /// Play a one-shot sound (e.g. a block being placed): a random variant at a random
    /// pitch, fire-and-forget. No-op if audio is disabled or the sound didn't decode.
    pub fn play(&mut self, sound: Sound) {
        self.emit(sound);
    }

    /// Play `sound` once with fresh random pitch and its gain, returning how long
    /// (wall clock) the play will take — read from the clip, so it adapts to any
    /// asset. `0.0` if the sound has no decoded buffer. A no-op for output when audio
    /// is disabled, but it still returns the duration so loop timing stays stable.
    fn emit(&mut self, sound: Sound) -> f64 {
        let def = sound.def();
        // How many decoded variants this sound has (immutable borrow, released here).
        let count = self.buffers.get(sound as usize).map_or(0, Vec::len);
        if count == 0 {
            return 0.0;
        }
        // All randomness up front — the only `&mut self` use, so the buffer borrow
        // below doesn't conflict: a random variant (so a repeated sound isn't the
        // same clip) plus the per-play pitch jitter.
        let variant = self.next_index(count);
        let pitch = 1.0 + self.next_jitter() * def.pitch_variation;
        let gain = self.master_gain * category_gain(def.category) * def.gain;

        let buf = &self.buffers[sound as usize][variant];
        // The faster it plays (higher pitch), the sooner it ends.
        let duration = buf.duration() / pitch.max(0.01) as f64;
        if let Some(sink) = self.sink.as_ref() {
            // Clone the decoded PCM into a fresh replayable source, shift its pitch
            // and gain, and mix it in. `speed` resamples (pitch + tempo together);
            // `add` overlaps it with anything already playing.
            let source = SamplesBuffer::new(buf.channels, buf.sample_rate, buf.samples.clone())
                .speed(pitch)
                .amplify(gain);
            sink.mixer().add(source);
        }
        duration
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
        SoundCategory::Block | SoundCategory::Ui => 1.0,
    }
}

/// Decode OGG/Vorbis `bytes` into an in-memory PCM buffer (f32 samples + format).
/// Device-free and split out so it is unit-testable without an audio device.
fn decode(bytes: &'static [u8]) -> Result<DecodedSound, String> {
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
            loop_sound: None,
            loop_next: 0.0,
        }
    }

    #[test]
    fn wood_punch_variant_decodes_to_pcm() {
        // A variant decodes to real PCM with no audio device involved — proving the
        // embed + decode path. Format isn't pinned (freely-edited asset data): we
        // only require a sane, playable buffer with a real duration.
        let d = decode(SOUND_DEFS[Sound::WoodPunch as usize].variants[0])
            .expect("wood_punch variant should decode");
        assert!(!d.samples.is_empty(), "decoded to some samples");
        assert!(d.sample_rate.get() > 0);
        assert!(d.channels.get() >= 1);
        assert!(d.duration() > 0.0, "has a positive duration");
    }

    #[test]
    fn every_sound_has_at_least_one_variant() {
        // A sound with no clips would be silently silent — a data mistake, not a
        // tunable value, so this guards the structure without pinning the count.
        for def in SOUND_DEFS {
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
}
