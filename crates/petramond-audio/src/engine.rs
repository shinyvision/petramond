//! The rodio-backed playback engine — the `audio` feature's half of the
//! module. See `audio/mod.rs` (the always-compiled registry + shared types)
//! and `audio/engine_off.rs` (the featureless silent stub with the same
//! surface).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use rodio::mixer::Mixer;
use rodio::source::Source;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, SampleRate, SpatialPlayer};

use super::keep_alive::KeepAlive;
use super::{MusicTrack, Sound, SoundCategory, SpatialListener, SpatialSoundSource};
use offline::WorldOutput;
use petramond_world::sound_registry::defs as sound_defs;

/// Mining punch sounds retrigger at a fixed cadence while held. Each trigger is a
/// one-shot mixed over any previous trigger, so long clips can overlap naturally.
const MINING_REPEAT_INTERVAL: f64 = 0.300;
const EAR_HALF_SPACING: f32 = 0.18;

/// Seconds a music track takes to fade to silence when it is stopped early
/// (leaving a world, the volume slider reaching zero). A track that plays to
/// its own end never fades — it finishes as it was mastered.
const MUSIC_FADE_SECONDS: f32 = 1.5;

mod offline;
mod tap;
mod voice;

/// A sound decoded into memory once at startup. Its PCM is shared: every
/// voice reads the same `Arc<[f32]>` through its own cursor (`voice`), so a
/// play is a reference-count bump, never a copy of the samples.
struct DecodedSound {
    channels: ChannelCount,
    sample_rate: SampleRate,
    samples: Arc<[f32]>,
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

impl SpatialListener {
    fn audio_space(
        self,
        emitter: petramond_math::world_pos::WorldPos,
        attenuation_distance: f32,
    ) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let scale = attenuation_distance.max(1.0);
        let emitter = (emitter - self.pos) / scale;
        let right = self.right.normalize_or_zero() * EAR_HALF_SPACING;
        // rodio's `Spatial` makes the FARTHER ear the louder one, so each
        // channel's "ear" sits on the opposite side of the head.
        (
            vec3(emitter),
            vec3(petramond_math::math::Vec3::ZERO + right),
            vec3(petramond_math::math::Vec3::ZERO - right),
        )
    }
}

struct ActiveSpatialSound {
    sink: SpatialPlayer,
    sound: Sound,
    /// The decoded clip it plays and how far into it, so a sound moved to
    /// another mixer (see `Audio::rebind_world_voices`) goes on from there.
    variant: usize,
    cursor: voice::ClipCursor,
    source: SpatialSoundSource,
    /// The sound's own gain (row gain × caller volume) EXCLUDING the mixer
    /// volumes, which are re-read live every [`Audio::update_spatial`] so a
    /// slider move mid-play takes effect.
    local_gain: f32,
    pitch: f32,
    /// Where the emitter was last written to the sink.
    applied: petramond_math::world_pos::WorldPos,
    /// Where [`Audio::update_spatial`] last resolved it — for a mob-pinned
    /// sound whose mob is gone, also where it finishes.
    target: petramond_math::world_pos::WorldPos,
}

/// A world one-shot still sounding: kept so a change of world output can
/// carry it over from where it is (see `Audio::rebind_world_voices`).
struct ActiveOneShot {
    sink: rodio::Player,
    shot: OneShot,
    cursor: voice::ClipCursor,
}

/// One play of a clip: which sound, which of its variants, at what pitch and
/// with what gain on top of the row's own.
#[derive(Copy, Clone)]
struct OneShot {
    sound: Sound,
    variant: usize,
    pitch: f32,
    extra_gain: f32,
}

/// Which mixer a sound joins. The WORLD lane is everything the session's
/// world sounds like, and follows the world output onto an offline mixdown;
/// the INTERFACE lane is the viewer's own chrome and soundtrack, which stays
/// on the device and is never part of a mixdown.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Lane {
    World,
    Interface,
}

/// The three mixer sliders (each `0..=1`).
#[derive(Copy, Clone, Debug)]
struct Volumes {
    master: f32,
    sound: f32,
    music: f32,
}

impl Volumes {
    /// The live mixer gain for `category`: master × its volume group.
    fn gain(self, category: SoundCategory) -> f32 {
        let group = match category {
            SoundCategory::Music => self.music,
            _ => self.sound,
        };
        self.master * group
    }
}

/// The audio engine: owns the output stream and the decoded sound buffers, and
/// drives at most one repeated sound (e.g. mining). Lives on the client (the `App`);
/// the simulation never touches it.
pub struct Audio {
    /// OS output stream + mixer. `None` when no device opened. Kept alive for
    /// the lifetime of `Audio`: dropping it stops all playback.
    device: Option<MixerDeviceSink>,
    /// The world lane's own mixer on the device, copied for taps; `None`
    /// with no device.
    submix: Option<tap::WorldSubmix>,
    /// Where the world lane mixes: the device, or an offline mixdown.
    world: WorldOutput,
    /// Decoded variant buffers per sound, indexed by raw sound id (parallel to
    /// the loaded sound table). Each sound holds a list of interchangeable clips; a play
    /// picks one at random. An empty list (all variants failed to decode) is silent.
    buffers: Vec<Vec<DecodedSound>>,
    /// Options → Sound: master over everything, sound over every non-music
    /// category, music over the `music` category.
    volumes: Volumes,
    /// The world lane's per-play variant + pitch jitter. Presentation-only
    /// randomness: seeded from the wall clock live so runs differ, from the
    /// caller's seed offline so a mixdown reproduces.
    rng: Xorshift,
    /// The interface lane's own stream, so a menu click never shifts the
    /// world lane's seeded sequence.
    interface_rng: Xorshift,
    #[cfg(any(test, feature = "test-support"))]
    played: Vec<Sound>,
    /// The sound currently repeating (e.g. mining) and the wall-clock time its next
    /// trigger is due. Driven per-frame by [`set_loop`](Self::set_loop).
    loop_sound: Option<Sound>,
    loop_next: f64,
    spatial: HashMap<u64, ActiveSpatialSound>,
    /// World one-shots still sounding, in the order they started.
    oneshots: Vec<ActiveOneShot>,
    /// The listener the spatial sounds were last written for, and the one
    /// [`update_spatial`](Self::update_spatial) last asked for; an offline
    /// pull moves from one to the other in slices.
    listener_applied: Option<SpatialListener>,
    listener_target: Option<SpatialListener>,
    /// The world is frozen (singleplayer pause): every active spatial sound
    /// holds its place, and one started meanwhile starts held.
    spatial_paused: bool,
    /// Gain-controlled continuous loops driven by client mods
    /// (`ClientLoopSet`): resolved sound → its infinite sink + eased gain.
    gain_loops: HashMap<Sound, ActiveGainLoop>,
    /// The music channel: at most one track sounding at a time. `None` between
    /// tracks. Deliberately outside the spatial table — music has no place in
    /// the world, so it neither attenuates nor freezes with a paused world.
    music: Option<ActiveMusic>,
}

/// The music channel's current track. Unlike every other sound, it is
/// STREAMED: the sink holds a decoder over the compressed file and pulls PCM
/// on the audio thread, so a three-minute piece costs its few megabytes of
/// OGG rather than the tens of megabytes its PCM would.
struct ActiveMusic {
    sink: rodio::Player,
    track: MusicTrack,
    /// Linear 0..=1 envelope over the row gain, ramped down by
    /// [`Audio::stop_music`] so a track never cuts mid-phrase.
    envelope: f32,
    fading_out: bool,
}

/// One continuous mod-driven loop: an infinite repeating sink whose volume
/// eases toward the mod's requested gain (ambience must never pop).
struct ActiveGainLoop {
    sink: rodio::Player,
    cursor: voice::ClipCursor,
    gain: f32,
    target: f32,
}

impl Audio {
    /// Open the default audio device and decode every sound. Best-effort: failing to
    /// open the device (headless / no speaker) or to decode a sound logs a warning
    /// and leaves that part silent — never an error.
    pub fn new() -> Self {
        let device = match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                // Keep the OS audio device — and especially a Bluetooth link — awake
                // for the whole session by mixing in a continuous inaudible signal, so
                // the first sound after a silent gap isn't delayed by a 200-500 ms
                // device cold-start (see `keep_alive`). Device only: an offline
                // mixdown would bake the noise into every export.
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
        Self::with_device(device, seed_rng())
    }

    fn with_device(device: Option<MixerDeviceSink>, seed: u64) -> Self {
        let submix = device.as_ref().map(|sink| {
            let cfg = sink.config();
            let (submix, source) = tap::WorldSubmix::new(cfg.channel_count(), cfg.sample_rate());
            sink.mixer().add(source);
            submix
        });
        let buffers = sound_defs()
            .iter()
            .map(|def| {
                def.variants
                    .iter()
                    .filter_map(|&rel| {
                        let Some((bytes, _)) = petramond_world::assets::read_bytes(rel) else {
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
            device,
            submix,
            world: WorldOutput::Device,
            buffers,
            volumes: Volumes {
                master: 1.0,
                sound: 1.0,
                music: 1.0,
            },
            rng: Xorshift::new(seed),
            interface_rng: Xorshift::new(seed_rng()),
            #[cfg(any(test, feature = "test-support"))]
            played: Vec::new(),
            loop_sound: None,
            loop_next: 0.0,
            spatial: HashMap::new(),
            oneshots: Vec::new(),
            listener_applied: None,
            listener_target: None,
            spatial_paused: false,
            gain_loops: HashMap::new(),
            music: None,
        }
    }

    /// The slider volumes a play on `lane` mixes at. An offline mixdown is
    /// the world as it sounds, not as this viewer listens: the world lane
    /// then ignores the sliders.
    fn lane_volumes(&self, lane: Lane) -> Volumes {
        match lane {
            Lane::World if self.world.is_offline() => Volumes {
                master: 1.0,
                sound: 1.0,
                music: 1.0,
            },
            _ => self.volumes,
        }
    }

    /// The mixer `lane` joins, or `None` when it has nowhere to sound.
    fn mixer(&self, lane: Lane) -> Option<&Mixer> {
        match (lane, &self.world) {
            (Lane::World, WorldOutput::Offline(mix)) => Some(mix.mixer()),
            (Lane::World, WorldOutput::Device) => self.submix.as_ref().map(tap::WorldSubmix::mixer),
            (Lane::Interface, _) => self.device.as_ref().map(MixerDeviceSink::mixer),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn take_played_for_test(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.played)
    }

    /// Set the mixer volumes (each `0..=1`): master over everything, `sound`
    /// over every non-music category, `music` over the `music` category.
    /// One-shots pick the new gains up at their next play; active spatial
    /// sounds re-read them on the next per-frame update.
    pub fn set_volumes(&mut self, master: f32, sound: f32, music: f32) {
        self.volumes = Volumes {
            master: master.clamp(0.0, 1.0),
            sound: sound.clamp(0.0, 1.0),
            music: music.clamp(0.0, 1.0),
        };
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
                    self.emit(s, 1.0, Lane::World);
                    self.loop_sound = Some(s);
                    // Stepped from the due time, not from `now`, so the cadence
                    // does not stretch to whole frames at a low offline frame rate.
                    let next = self.loop_next + MINING_REPEAT_INTERVAL;
                    self.loop_next = if changed || next <= now {
                        now + MINING_REPEAT_INTERVAL
                    } else {
                        next
                    };
                }
            }
        }
    }

    /// Sync the client-mod continuous loops to `desired` `(sound, gain)`
    /// rows and ease every active loop's volume over `dt` seconds. A sound
    /// absent from `desired` (or at gain 0) eases to silence and stops. The
    /// clip repeats seamlessly (variant 0 — loop assets are single-variant);
    /// mixer sliders apply live like every other sound.
    pub fn update_gain_loops(&mut self, desired: &[(Sound, f32)], dt: f32) {
        /// Seconds for a gain change to close ~63% of its gap.
        const LOOP_EASE_SECONDS: f32 = 1.0;
        /// Below this eased gain a silenced loop stops and is dropped.
        const LOOP_DEAD_GAIN: f32 = 0.005;
        let Some(mixer) = self.mixer(Lane::World).cloned() else {
            return;
        };
        for &(sound, gain) in desired {
            let gain = gain.clamp(0.0, 4.0);
            if let Some(active) = self.gain_loops.get_mut(&sound) {
                active.target = gain;
                continue;
            }
            if gain <= 0.0 {
                continue;
            }
            let Some(buf) = self
                .buffers
                .get(sound.0 as usize)
                .and_then(|variants| variants.first())
            else {
                continue;
            };
            let (sink, cursor) = connect_gain_loop(&mixer, buf, 0.0, 0);
            self.gain_loops.insert(
                sound,
                ActiveGainLoop {
                    sink,
                    cursor,
                    gain: 0.0,
                    target: gain,
                },
            );
        }
        // Any active loop no longer desired eases to silence.
        for (sound, active) in self.gain_loops.iter_mut() {
            if !desired.iter().any(|(s, _)| s == sound) {
                active.target = 0.0;
            }
        }
        let ease = 1.0 - (-dt.clamp(0.0, 0.25) / LOOP_EASE_SECONDS).exp();
        let volumes = self.lane_volumes(Lane::World);
        self.gain_loops.retain(|sound, active| {
            active.gain += (active.target - active.gain) * ease;
            if active.target <= 0.0 && active.gain < LOOP_DEAD_GAIN {
                active.sink.stop();
                return false;
            }
            let def = sound.def();
            active
                .sink
                .set_volume(volumes.gain(def.category) * def.gain * active.gain);
            true
        });
    }

    /// Stop every mod loop immediately (session teardown — no ease; the
    /// world the loops belonged to is gone).
    pub fn stop_gain_loops(&mut self) {
        for (_, active) in self.gain_loops.drain() {
            active.sink.stop();
        }
    }

    /// Start `track` on the music channel, replacing whatever was playing.
    /// Returns whether it actually started — a missing or undecodable file
    /// answers `false` so the scheduler can move on to another track instead
    /// of waiting out a silent piece.
    ///
    /// The clip is read and decoded HERE, not at startup: music is streamed
    /// (see [`ActiveMusic`]).
    ///
    /// The soundtrack is the viewer's, scheduled on their own clock rather
    /// than recorded with the session, so it rides the interface lane and
    /// never reaches an offline mixdown.
    pub fn play_music(&mut self, track: MusicTrack) -> bool {
        let Some(mixer) = self.mixer(Lane::Interface) else {
            return false;
        };
        let def = track.def();
        let Some((bytes, _)) = petramond_world::assets::read_bytes(def.file) else {
            log::warn!("music {track:?} clip '{}' not found", def.file);
            return false;
        };
        let decoder = match rodio::Decoder::try_from(Cursor::new(bytes)) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("music {track:?} failed to decode: {e}");
                return false;
            }
        };
        let player = rodio::Player::connect_new(mixer);
        player.set_volume(self.volumes.gain(SoundCategory::Music) * def.gain);
        player.append(decoder);
        if let Some(replaced) = self.music.replace(ActiveMusic {
            sink: player,
            track,
            envelope: 1.0,
            fading_out: false,
        }) {
            replaced.sink.stop();
        }
        true
    }

    /// Fade the current track out and drop it. Inert when nothing is playing
    /// or the fade already started, so a caller may say it every frame.
    pub fn stop_music(&mut self) {
        if let Some(music) = self.music.as_mut() {
            music.fading_out = true;
        }
    }

    /// The track sounding right now, or `None` between tracks. A track that
    /// has been asked to stop is already `None` here: the scheduler's next gap
    /// starts when the music ENDS, not when its fade finishes.
    pub fn music_playing(&self) -> Option<MusicTrack> {
        self.music
            .as_ref()
            .filter(|m| !m.fading_out)
            .map(|m| m.track)
    }

    /// Advance the music channel by `dt` seconds: run any fade, re-read the
    /// mixer volumes (so a slider drag is live, like every other sound), and
    /// retire a track that has finished. Call every frame.
    pub fn update_music(&mut self, dt: f32) {
        // Read out before the mutable borrow: the channel re-reads the mixer
        // volumes every frame, so a slider drag is live mid-track.
        let mix = self.volumes.gain(SoundCategory::Music);
        let Some(music) = self.music.as_mut() else {
            return;
        };
        if music.fading_out {
            music.envelope -= dt.clamp(0.0, 0.25) / MUSIC_FADE_SECONDS;
        }
        // A finished track retires itself: `Player::empty` is the only honest
        // signal that a streamed decoder ran out.
        if music.envelope <= 0.0 || music.sink.empty() {
            music.sink.stop();
            self.music = None;
            return;
        }
        music
            .sink
            .set_volume(mix * music.track.def().gain * music.envelope);
    }

    /// Play a one-shot sound (e.g. a block being placed): a random variant at a random
    /// pitch, fire-and-forget. No-op if audio is disabled or the sound didn't decode.
    pub fn play(&mut self, sound: Sound) {
        self.emit(sound, 1.0, Lane::World);
    }

    /// [`play`](Self::play) for the viewer's own interface (a menu click):
    /// always on the device, never part of an offline mixdown.
    pub fn play_interface(&mut self, sound: Sound) {
        self.emit(sound, 1.0, Lane::Interface);
    }

    /// [`play`](Self::play) with an extra linear gain factor on top of the
    /// sound's own — the distance-attenuation hook for positional (mod-emitted)
    /// sounds. A non-positive gain skips the play entirely.
    pub fn play_attenuated(&mut self, sound: Sound, gain: f32) {
        if gain > 0.0 {
            self.emit(sound, gain, Lane::World);
        }
    }

    /// Start or replace an active spatial sound. No-op when audio is disabled,
    /// the sound has no decoded variants, or the handle is zero.
    #[allow(clippy::too_many_arguments)]
    pub fn play_spatial(
        &mut self,
        handle: u64,
        sound: Sound,
        source: SpatialSoundSource,
        volume: f32,
        pitch: f32,
        listener: SpatialListener,
        initial_position: petramond_math::world_pos::WorldPos,
    ) {
        if handle == 0 || volume <= 0.0 || pitch <= 0.0 {
            return;
        }
        let count = self.buffers.get(sound.0 as usize).map_or(0, Vec::len);
        if count == 0 {
            return;
        }
        let variant = self.rng.index(count);
        let Some(mixer) = self.mixer(Lane::World) else {
            return;
        };
        let def = sound.def();
        let local_gain = def.gain * volume;
        let (player, cursor) =
            self.connect_spatial(mixer, sound, variant, listener, initial_position, 0);
        player.set_volume(
            self.lane_volumes(Lane::World).gain(def.category)
                * local_gain
                * sound.distance_gain((initial_position - listener.pos).length()),
        );
        player.set_speed(pitch);
        if self.listener_applied.is_none() {
            self.listener_applied = Some(listener);
            self.listener_target = Some(listener);
        }
        if let Some(replaced) = self.spatial.insert(
            handle,
            ActiveSpatialSound {
                sink: player,
                sound,
                variant,
                cursor,
                source,
                local_gain,
                pitch,
                applied: initial_position,
                target: initial_position,
            },
        ) {
            replaced.sink.stop();
        }
    }

    /// A spatial player on `mixer` sounding `variant` of `sound` from clip
    /// frame `from` on, at `position` as `listener` hears it, held if the
    /// world is frozen. A loop row plays until `stop_spatial`;
    /// `update_spatial`'s finished-sink sweep never sees it empty.
    fn connect_spatial(
        &self,
        mixer: &Mixer,
        sound: Sound,
        variant: usize,
        listener: SpatialListener,
        position: petramond_math::world_pos::WorldPos,
        from: usize,
    ) -> (SpatialPlayer, voice::ClipCursor) {
        let def = sound.def();
        let buf = &self.buffers[sound.0 as usize][variant];
        let (emitter, left_ear, right_ear) =
            listener.audio_space(position, def.attenuation_distance);
        let player = SpatialPlayer::connect_new(mixer, emitter, left_ear, right_ear);
        let (source, cursor) = voice::clip_from(buf, from, def.looped);
        player.append(source);
        if self.spatial_paused {
            player.pause();
        }
        (player, cursor)
    }

    /// Retune a live spatial sound's own gain and pitch in place (`SoundSet`);
    /// the next per-frame update applies them with the mixer and distance
    /// terms. Unknown or finished handles are inert.
    pub fn set_spatial(&mut self, handle: u64, volume: f32, pitch: f32) {
        if !(volume.is_finite() && volume >= 0.0 && pitch.is_finite() && pitch > 0.0) {
            return;
        }
        if let Some(active) = self.spatial.get_mut(&handle) {
            active.local_gain = active.sound.def().gain * volume;
            active.pitch = pitch;
        }
    }

    /// Start a presentation-owned one-shot spatial sound using the row's own
    /// gain and pitch jitter. This is for engine presentation events such as
    /// mob hurt/death calls; deterministic mod HostCalls keep using
    /// [`play_spatial`](Self::play_spatial), where the guest supplies pitch.
    pub fn play_spatial_randomized(
        &mut self,
        handle: u64,
        sound: Sound,
        source: SpatialSoundSource,
        listener: SpatialListener,
        initial_position: petramond_math::world_pos::WorldPos,
    ) {
        let def = sound.def();
        let pitch = def.pitch * (1.0 + self.rng.jitter() * def.pitch_variation);
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

    /// Hold every active spatial sound where it is (`true`) or let it run
    /// on (`false`): the world's sounds freeze with a frozen world — a
    /// singleplayer pause — and resume from the same place. Call every
    /// frame; only a change touches the sinks.
    pub fn set_spatial_paused(&mut self, paused: bool) {
        if self.spatial_paused == paused {
            return;
        }
        self.spatial_paused = paused;
        for active in self.spatial.values() {
            if paused {
                active.sink.pause();
            } else {
                active.sink.play();
            }
        }
    }

    /// Stop a spatial sound. Unknown handles are intentionally inert.
    pub fn stop_spatial(&mut self, handle: u64) {
        if let Some(active) = self.spatial.remove(&handle) {
            active.sink.stop();
        }
    }

    pub fn clear_spatial(&mut self) {
        for (_, active) in self.spatial.drain() {
            active.sink.stop();
        }
    }

    /// Refresh active spatial sounds from the current camera and the same
    /// per-frame mob positions the renderer consumes. A mob-pinned sound whose
    /// mob id is absent keeps its last position and is allowed to finish there.
    ///
    /// On the device the new positions are written at once. On an offline
    /// mixdown they are the frame's TARGET, reached slice by slice through the
    /// next [`pull`](Self::pull).
    pub fn update_spatial(
        &mut self,
        listener: SpatialListener,
        mobs: &[(u64, petramond_math::world_pos::WorldPos)],
    ) {
        if self.mixer(Lane::World).is_none() {
            self.spatial.clear();
            self.oneshots.clear();
            return;
        }
        self.oneshots.retain(|shot| !shot.sink.empty());
        self.listener_target = Some(listener);
        for active in self.spatial.values_mut() {
            if let SpatialSoundSource::Mob(id) = active.source {
                if let Some((_, pos)) = mobs.iter().find(|(mob_id, _)| *mob_id == id) {
                    active.target = *pos;
                }
            }
        }
        if !self.world.is_offline() {
            self.apply_spatial(1.0);
        }
        self.spatial.retain(|_, active| !active.sink.empty());
    }

    /// Write every spatial sound's controls at fraction `t` of the way from
    /// what was last written to what [`update_spatial`](Self::update_spatial)
    /// last asked for; `t = 1` lands on the target and commits it.
    fn apply_spatial(&mut self, t: f32) {
        let Some(to) = self.listener_target else {
            return;
        };
        let listener = self.listener_applied.unwrap_or(to).toward(to, t);
        let volumes = self.lane_volumes(Lane::World);
        for active in self.spatial.values_mut() {
            let pos = toward(active.applied, active.target, t);
            let def = active.sound.def();
            let (emitter, left_ear, right_ear) =
                listener.audio_space(pos, def.attenuation_distance);
            active.sink.set_emitter_position(emitter);
            active.sink.set_left_ear_position(left_ear);
            active.sink.set_right_ear_position(right_ear);
            active.sink.set_volume(
                volumes.gain(def.category)
                    * active.local_gain
                    * active.sound.distance_gain((pos - listener.pos).length()),
            );
            active.sink.set_speed(active.pitch);
            if t >= 1.0 {
                active.applied = active.target;
            }
        }
        if t >= 1.0 {
            self.listener_applied = Some(to);
        }
    }

    /// Play `sound` once on `lane` with fresh random pitch and its gain (scaled
    /// by `extra_gain`). No-op if audio is disabled or the sound didn't decode.
    fn emit(&mut self, sound: Sound, extra_gain: f32, lane: Lane) {
        let def = sound.def();
        let count = self.buffers.get(sound.0 as usize).map_or(0, Vec::len);
        if count == 0 {
            return;
        }
        #[cfg(any(test, feature = "test-support"))]
        self.played.push(sound);
        // A random variant (so a repeated sound isn't the same clip) plus the
        // per-play pitch jitter, each lane from its own stream.
        let rng = match lane {
            Lane::World => &mut self.rng,
            Lane::Interface => &mut self.interface_rng,
        };
        let variant = rng.index(count);
        let pitch = def.pitch * (1.0 + rng.jitter() * def.pitch_variation);
        let shot = OneShot {
            sound,
            variant,
            pitch,
            extra_gain,
        };
        self.sound_one_shot(shot, lane, 0);
    }

    /// Sound `shot` on `lane` from clip frame `from` on. `speed` resamples
    /// (pitch and tempo together), and the player overlaps whatever is
    /// already playing. A world one-shot is kept until it ends, so it can
    /// follow the world output onto another mixer.
    fn sound_one_shot(&mut self, shot: OneShot, lane: Lane, from: usize) {
        let Some(mixer) = self.mixer(lane) else {
            return;
        };
        let def = shot.sound.def();
        let buf = &self.buffers[shot.sound.0 as usize][shot.variant];
        let player = rodio::Player::connect_new(mixer);
        player.set_speed(shot.pitch);
        player.set_volume(self.lane_volumes(lane).gain(def.category) * def.gain * shot.extra_gain);
        let (source, cursor) = voice::clip_from(buf, from, false);
        player.append(source);
        match lane {
            Lane::World => {
                self.oneshots.retain(|shot| !shot.sink.empty());
                self.oneshots.push(ActiveOneShot {
                    sink: player,
                    shot,
                    cursor,
                });
            }
            Lane::Interface => player.detach(),
        }
    }
}

/// An xorshift64 stream. Presentation-only randomness (variant + pitch),
/// never the deterministic worldgen RNG.
#[derive(Debug)]
struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    #[inline]
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Next pitch jitter in `[-1.0, 1.0)`.
    fn jitter(&mut self) -> f32 {
        // Top 24 bits → [0, 1) → [-1, 1).
        let unit = (self.next() >> 40) as f32 / (1u32 << 24) as f32;
        unit * 2.0 - 1.0
    }

    /// A uniform-ish random index in `[0, len)` (returns 0 when `len <= 1`). The
    /// modulo bias is negligible for the handful of variants a sound has.
    fn index(&mut self, len: usize) -> usize {
        if len <= 1 {
            return 0;
        }
        (self.next() % len as u64) as usize
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

fn vec3(v: petramond_math::math::Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// A mod loop's endless player on `mixer`, at `volume`, from clip frame
/// `from` on.
fn connect_gain_loop(
    mixer: &Mixer,
    buf: &DecodedSound,
    volume: f32,
    from: usize,
) -> (rodio::Player, voice::ClipCursor) {
    let player = rodio::Player::connect_new(mixer);
    player.set_volume(volume);
    let (source, cursor) = voice::clip_from(buf, from, true);
    player.append(source);
    (player, cursor)
}

/// No body the world simulates covers this many blocks in one frame: a jump
/// past it is a teleport, a respawn or a seek, and snaps instead of sweeping
/// audibly through the space between.
const SNAP_DISTANCE: f32 = 8.0;

/// `t` of the way from `from` to `to`, exactly `to` at `t >= 1`.
fn toward(
    from: petramond_math::world_pos::WorldPos,
    to: petramond_math::world_pos::WorldPos,
    t: f32,
) -> petramond_math::world_pos::WorldPos {
    if t >= 1.0 || (to - from).length() > SNAP_DISTANCE {
        to
    } else {
        from.lerp(to, t)
    }
}

impl SpatialListener {
    fn toward(self, to: SpatialListener, t: f32) -> SpatialListener {
        let right = self.right.lerp(to.right, t.clamp(0.0, 1.0));
        SpatialListener {
            pos: toward(self.pos, to.pos, t),
            // A half turn inside one frame passes through a zero ear axis,
            // which puts both ears in one place.
            right: if right.length_squared() > 1e-6 {
                right
            } else {
                to.right
            },
        }
    }
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
        samples: samples.into(),
    })
}

/// A non-deterministic, presentation-only seed for the pitch-jitter RNG, from the
/// wall clock so different live runs vary. The fixed fallback keeps it infallible.
fn seed_rng() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device-less engine for exercising the pure RNG helpers.
    fn silent_audio(seed: u64) -> Audio {
        Audio {
            device: None,
            submix: None,
            world: WorldOutput::Device,
            buffers: Vec::new(),
            volumes: Volumes {
                master: 1.0,
                sound: 1.0,
                music: 1.0,
            },
            rng: Xorshift::new(seed),
            interface_rng: Xorshift::new(seed),
            played: Vec::new(),
            loop_sound: None,
            loop_next: 0.0,
            spatial: HashMap::new(),
            oneshots: Vec::new(),
            listener_applied: None,
            listener_target: None,
            spatial_paused: false,
            gain_loops: HashMap::new(),
            music: None,
        }
    }

    #[test]
    fn wood_punch_variant_decodes_to_pcm() {
        // A variant decodes to real PCM with no audio device involved — proving the
        // embed + decode path. Format isn't pinned (freely-edited asset data): we
        // only require a sane, playable buffer with a real duration.
        let rel = sound_defs()[Sound::WoodPunch.0 as usize].variants[0];
        let bytes = petramond_world::assets::read_bytes(rel)
            .expect("clip file exists")
            .0;
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
            let j = a.rng.jitter();
            assert!((-1.0..1.0).contains(&j), "jitter {j} out of range");
        }
    }

    #[test]
    fn random_variant_stays_in_range_and_covers_all() {
        let mut a = silent_audio(0xDEAD_BEEF_CAFE_1234);
        let mut seen = [false; 3];
        for _ in 0..1_000 {
            let i = a.rng.index(3);
            assert!(i < 3, "index {i} out of range");
            seen[i] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "every variant should be chosen over many plays"
        );
        // Degenerate counts never panic or index out of bounds.
        assert_eq!(a.rng.index(1), 0);
        assert_eq!(a.rng.index(0), 0);
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
