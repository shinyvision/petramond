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

const MINING_REPEAT_INTERVAL: f64 = 0.300;
const EAR_HALF_SPACING: f32 = 0.18;

const MUSIC_FADE_SECONDS: f32 = 1.5;

mod offline;
mod tap;
mod voice;

struct DecodedSound {
    channels: ChannelCount,
    sample_rate: SampleRate,
    samples: Arc<[f32]>,
}

impl DecodedSound {
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
    variant: usize,
    cursor: voice::ClipCursor,
    source: SpatialSoundSource,
    local_gain: f32,
    pitch: f32,
    applied: petramond_math::world_pos::WorldPos,
    target: petramond_math::world_pos::WorldPos,
}

struct ActiveOneShot {
    sink: rodio::Player,
    shot: OneShot,
    cursor: voice::ClipCursor,
}

#[derive(Copy, Clone)]
struct OneShot {
    sound: Sound,
    variant: usize,
    pitch: f32,
    extra_gain: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Lane {
    World,
    Interface,
}

#[derive(Copy, Clone, Debug)]
struct Volumes {
    master: f32,
    sound: f32,
    music: f32,
}

impl Volumes {
    fn gain(self, category: SoundCategory) -> f32 {
        let group = match category {
            SoundCategory::Music => self.music,
            _ => self.sound,
        };
        self.master * group
    }
}

pub struct Audio {
    device: Option<MixerDeviceSink>,
    submix: Option<tap::WorldSubmix>,
    world: WorldOutput,
    buffers: Vec<Vec<DecodedSound>>,
    volumes: Volumes,
    rng: Xorshift,
    interface_rng: Xorshift,
    #[cfg(any(test, feature = "test-support"))]
    played: Vec<Sound>,
    loop_sound: Option<Sound>,
    loop_next: f64,
    spatial: HashMap<u64, ActiveSpatialSound>,
    oneshots: Vec<ActiveOneShot>,
    listener_applied: Option<SpatialListener>,
    listener_target: Option<SpatialListener>,
    spatial_paused: bool,
    gain_loops: HashMap<Sound, ActiveGainLoop>,
    music: Option<ActiveMusic>,
}

struct ActiveMusic {
    sink: rodio::Player,
    track: MusicTrack,
    envelope: f32,
    fading_out: bool,
}

struct ActiveGainLoop {
    sink: rodio::Player,
    cursor: voice::ClipCursor,
    gain: f32,
    target: f32,
}

impl Audio {
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

    pub fn set_volumes(&mut self, master: f32, sound: f32, music: f32) {
        self.volumes = Volumes {
            master: master.clamp(0.0, 1.0),
            sound: sound.clamp(0.0, 1.0),
            music: music.clamp(0.0, 1.0),
        };
    }

    pub fn set_loop(&mut self, sound: Option<Sound>, now: f64) {
        match sound {
            None => self.loop_sound = None,
            Some(s) => {
                let changed = self.loop_sound != Some(s);
                if changed || now >= self.loop_next {
                    self.emit(s, 1.0, Lane::World);
                    self.loop_sound = Some(s);
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

    pub fn update_gain_loops(&mut self, desired: &[(Sound, f32)], dt: f32) {
        const LOOP_EASE_SECONDS: f32 = 1.0;
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

    pub fn stop_gain_loops(&mut self) {
        for (_, active) in self.gain_loops.drain() {
            active.sink.stop();
        }
    }

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

    pub fn stop_music(&mut self) {
        if let Some(music) = self.music.as_mut() {
            music.fading_out = true;
        }
    }

    pub fn music_playing(&self) -> Option<MusicTrack> {
        self.music
            .as_ref()
            .filter(|m| !m.fading_out)
            .map(|m| m.track)
    }

    pub fn update_music(&mut self, dt: f32) {
        let mix = self.volumes.gain(SoundCategory::Music);
        let Some(music) = self.music.as_mut() else {
            return;
        };
        if music.fading_out {
            music.envelope -= dt.clamp(0.0, 0.25) / MUSIC_FADE_SECONDS;
        }
        if music.envelope <= 0.0 || music.sink.empty() {
            music.sink.stop();
            self.music = None;
            return;
        }
        music
            .sink
            .set_volume(mix * music.track.def().gain * music.envelope);
    }

    pub fn play(&mut self, sound: Sound) {
        self.emit(sound, 1.0, Lane::World);
    }

    pub fn play_interface(&mut self, sound: Sound) {
        self.emit(sound, 1.0, Lane::Interface);
    }

    pub fn play_attenuated(&mut self, sound: Sound, gain: f32) {
        if gain > 0.0 {
            self.emit(sound, gain, Lane::World);
        }
    }

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

    pub fn set_spatial(&mut self, handle: u64, volume: f32, pitch: f32) {
        if !(volume.is_finite() && volume >= 0.0 && pitch.is_finite() && pitch > 0.0) {
            return;
        }
        if let Some(active) = self.spatial.get_mut(&handle) {
            active.local_gain = active.sound.def().gain * volume;
            active.pitch = pitch;
        }
    }

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

    fn emit(&mut self, sound: Sound, extra_gain: f32, lane: Lane) {
        let def = sound.def();
        let count = self.buffers.get(sound.0 as usize).map_or(0, Vec::len);
        if count == 0 {
            return;
        }
        #[cfg(any(test, feature = "test-support"))]
        self.played.push(sound);
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

    fn jitter(&mut self) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1u32 << 24) as f32;
        unit * 2.0 - 1.0
    }

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

const SNAP_DISTANCE: f32 = 8.0;

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
            right: if right.length_squared() > 1e-6 {
                right
            } else {
                to.right
            },
        }
    }
}

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
