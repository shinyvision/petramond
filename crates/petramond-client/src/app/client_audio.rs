//! The app's sound orchestration: the audio engine, the soundtrack director,
//! and the client-owned bookkeeping that turns a frame's game events and
//! presentation snapshot into plays — positional world cues, spatial sound
//! commands, mob idle cadence and footsteps.
//!
//! Never part of the deterministic simulation: the sim ships events, and this
//! decides when and where they are heard.

use std::collections::HashMap;

use petramond::mob::MobSoundCategory;
use petramond_audio::{Audio, SpatialListener, SpatialSoundSource};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::{Block, BlockSoundAction};
use petramond_world::sound_registry::Sound;

use super::music::MusicDirector;
use crate::animation::FootstepSource;
use crate::game::presentation::MobPresentation;
use crate::game::{GameEvents, MobSoundEvent, SpatialSoundCommand, WorldEvent};

/// The first client-local spatial handle: the top half of the handle space,
/// disjoint from the server-issued spatial sound handles.
const MOB_SOUND_HANDLE_START: u64 = 1 << 63;

/// Ticks between one body's footsteps, walking and sprinting. The gait comes
/// from presentation (which sees the body's real speed); the cadence is here.
const FOOTSTEP_INTERVAL_TICKS: u64 = 10;
const SPRINT_FOOTSTEP_INTERVAL_TICKS: u64 = 7;

/// Client-owned idle sound scheduling for one live mob.
#[derive(Default)]
pub(super) struct MobSoundState {
    pub(super) next_idle_tick: u64,
    pub(super) sequence: u64,
}

/// The app-lifetime half of sound: the engine and the soundtrack. Everything
/// that schedules or buffers a session's world sounds lives in the session's
/// [`SessionSounds`], which the per-frame calls borrow — so a new session can
/// never inherit the last one's cadences, cues or handle pool.
pub(super) struct ClientAudio {
    /// Client-side sound engine: plays the sim's sounds, never part of the
    /// deterministic simulation.
    audio: Audio,
    /// WHEN the soundtrack plays. Owns the gap between pieces and the choice
    /// of the next one; `audio` owns the streaming.
    music: MusicDirector,
}

/// The session-scoped sound bookkeeping between game events and plays, owned
/// by [`super::session::Session`] and dropped with it.
pub(super) struct SessionSounds {
    /// Spatial sound commands emitted by ticks since the last render, applied
    /// against the same mob presentation snapshot the renderer uses.
    spatial_commands: Vec<SpatialSoundCommand>,
    /// This render's interpolated mob positions, for pinning mob sounds.
    mob_positions: Vec<(u64, WorldPos)>,
    /// Gameplay-originated mob sound events waiting for the next presentation
    /// snapshot, where they can be pinned to interpolated mob positions.
    mob_events: Vec<MobSoundEvent>,
    /// Positional world-event one-shots (block place/break, doors, chest
    /// lids, foreign pickups) waiting for the next render's spatial listener.
    world_cues: Vec<(Sound, WorldPos)>,
    /// Idle sound scheduling per live mob session id.
    mob_states: HashMap<u64, MobSoundState>,
    /// Footstep cadence per walking body: the tick its next step is due.
    /// Retired with the bodies themselves each frame.
    footstep_next_tick: HashMap<u64, u64>,
    /// Reused per-frame scratch for client-mod loop gains (rain/wind beds).
    loop_gains: Vec<(Sound, f32)>,
    /// The next client-local spatial handle (wrapping within the top half).
    next_handle: u64,
}

impl Default for SessionSounds {
    fn default() -> Self {
        Self {
            spatial_commands: Vec::new(),
            mob_positions: Vec::new(),
            mob_events: Vec::new(),
            world_cues: Vec::new(),
            mob_states: HashMap::new(),
            footstep_next_tick: HashMap::new(),
            loop_gains: Vec::new(),
            next_handle: MOB_SOUND_HANDLE_START,
        }
    }
}

impl ClientAudio {
    pub(super) fn new(master: f32, sound: f32, music: f32) -> Self {
        let mut audio = Audio::new();
        audio.set_volumes(master, sound, music);
        Self {
            audio,
            music: MusicDirector::new(),
        }
    }

    /// Play a non-positional one-shot (UI clicks, the own pickup/hurt).
    pub(super) fn play(&mut self, sound: Sound) {
        self.audio.play(sound);
    }

    pub(super) fn set_volumes(&mut self, master: f32, sound: f32, music: f32) {
        self.audio.set_volumes(master, sound, music);
    }

    /// Stop the local mining loop (menus, pause, released buttons).
    pub(super) fn stop_mining_loop(&mut self, now: f64) {
        self.audio.set_loop(None, now);
    }

    /// One frame of session-level audio: spatial sounds freeze exactly when
    /// the world does, and the soundtrack belongs to the session — a frozen
    /// world lets the current track finish but schedules no new one.
    pub(super) fn update_session(&mut self, in_session: bool, world_frozen: bool, dt: f32) {
        self.audio.set_spatial_paused(world_frozen);
        self.music
            .update(&mut self.audio, in_session, world_frozen, dt);
    }

    /// The session ended: silence every voice it started in the engine — the
    /// mining loop, the client-mod ambience beds and every positional voice.
    /// Its scheduling state died with its [`SessionSounds`].
    pub(super) fn end_session(&mut self, now: f64) {
        self.audio.set_loop(None, now);
        self.audio.stop_gain_loops();
        self.audio.clear_spatial();
    }

    /// Queue one frame's game-event sounds: the mining loop follows
    /// `mining_block`, mod sounds play at once (attenuated by distance to the
    /// `listener` when positional), and spatial commands, mob sounds and
    /// positional world cues buffer in `sounds` for the next render, where
    /// the spatial listener exists.
    pub(super) fn queue_game_events(
        &mut self,
        sounds: &mut SessionSounds,
        events: &GameEvents,
        mining_block: Option<Block>,
        listener: Option<WorldPos>,
        now: f64,
    ) {
        let mining_sound = mining_block.and_then(|b| b.sound(BlockSoundAction::Dig));
        self.audio.set_loop(mining_sound, now);

        // Mod-emitted sounds (the non-lossy tick queue): each plays once,
        // attenuated by distance to the player when positional.
        for s in &events.sounds {
            let gain = match (s.pos, listener) {
                (Some(pos), Some(ear)) => s.sound.distance_gain((pos - ear).length()),
                _ => 1.0,
            };
            self.audio.play_attenuated(s.sound, gain);
        }
        sounds
            .spatial_commands
            .extend(events.spatial_sounds.iter().copied());
        sounds.mob_events.extend(events.mob_sounds.iter().copied());

        // World-anchored one-shots play POSITIONALLY, from the replicated
        // events (every observer hears them at the event's place — including
        // the local player's own actions).
        sounds
            .world_cues
            .extend(events.world_events.iter().filter_map(world_cue));

        if events.picked_up_item {
            self.audio.play(Sound::ItemPickup);
        }
        if events.player_damaged {
            self.audio.play(Sound::PlayerHurt);
        }
    }

    /// One render's positional audio against this frame's presentation: pin
    /// mob sounds to the interpolated `mobs`, apply the buffered spatial
    /// commands and cues, advance footsteps and idle cadence on the live
    /// world clock (`current_tick` — menus and multiplayer pause keep it
    /// moving), and ease the client-mod ambience loops `loop_gains` fills.
    pub(super) fn render_world(
        &mut self,
        sounds: &mut SessionSounds,
        frame: WorldAudioFrame<'_>,
        loop_gains: impl FnOnce(&mut Vec<(Sound, f32)>),
        dt: f32,
    ) {
        let listener = frame.listener;
        sounds.set_mob_positions(frame.mobs, frame.tick_alpha);
        let mut commands = std::mem::take(&mut sounds.spatial_commands);
        for command in commands.drain(..) {
            self.apply_spatial_command(sounds, command, listener);
        }
        sounds.spatial_commands = commands;
        let mut mob_events = std::mem::take(&mut sounds.mob_events);
        for event in mob_events.drain(..) {
            let Some(spec) = petramond::mob::def(event.kind).sound_for(event.category) else {
                continue;
            };
            let initial = sounds.mob_position(event.mob_id).unwrap_or(event.pos);
            self.play_mob_sound(sounds, spec.sound, event.mob_id, listener, initial);
        }
        sounds.mob_events = mob_events;
        // Positional world-event one-shots: fire-and-forget spatial plays off
        // the same client-local wrapping handle pool the mob sounds use.
        let mut cues = std::mem::take(&mut sounds.world_cues);
        for (sound, pos) in cues.drain(..) {
            let handle = sounds.alloc_handle();
            self.audio.play_spatial_randomized(
                handle,
                sound,
                SpatialSoundSource::Fixed(pos),
                listener,
                pos,
            );
        }
        sounds.world_cues = cues;
        self.tick_footsteps(sounds, listener, frame.footsteps, frame.current_tick);
        self.tick_idle_mob_sounds(sounds, listener, frame.mobs, frame.current_tick);
        self.audio.update_spatial(listener, &sounds.mob_positions);
        loop_gains(&mut sounds.loop_gains);
        self.audio.update_gain_loops(&sounds.loop_gains, dt);
    }

    fn apply_spatial_command(
        &mut self,
        sounds: &SessionSounds,
        command: SpatialSoundCommand,
        listener: SpatialListener,
    ) {
        match command {
            SpatialSoundCommand::PlayAt {
                handle,
                sound,
                pos,
                volume,
                pitch,
            } => self.audio.play_spatial(
                handle,
                sound,
                SpatialSoundSource::Fixed(pos),
                volume,
                pitch,
                listener,
                pos,
            ),
            SpatialSoundCommand::PlayOnMob {
                handle,
                sound,
                mob_id,
                volume,
                pitch,
                last_pos,
            } => {
                let initial = sounds.mob_position(mob_id).unwrap_or(last_pos);
                self.audio.play_spatial(
                    handle,
                    sound,
                    SpatialSoundSource::Mob(mob_id),
                    volume,
                    pitch,
                    listener,
                    initial,
                );
            }
            SpatialSoundCommand::Set {
                handle,
                volume,
                pitch,
            } => self.audio.set_spatial(handle, volume, pitch),
            SpatialSoundCommand::Stop { handle } => self.audio.stop_spatial(handle),
        }
    }

    /// Sound one footstep per walking body whose cadence is due.
    ///
    /// The presentation already decided WHO is walking and on WHAT; this owns
    /// only the cadence and the play. A body first seen walking steps
    /// IMMEDIATELY — its entry is seeded due. The map is keyed on the body,
    /// so a player who pauses resumes ON the same cadence instead of
    /// retriggering; entries die with the bodies (`footsteps` lists standing
    /// players too, which makes that retire exact).
    pub(super) fn tick_footsteps(
        &mut self,
        sounds: &mut SessionSounds,
        listener: SpatialListener,
        footsteps: &[FootstepSource],
        current_tick: u64,
    ) {
        for step in footsteps {
            let due = sounds
                .footstep_next_tick
                .entry(step.id)
                .or_insert(current_tick);
            let Some(ground) = step.ground else {
                continue;
            };
            if current_tick < *due {
                continue;
            }
            *due = current_tick.saturating_add(if step.sprinting {
                SPRINT_FOOTSTEP_INTERVAL_TICKS
            } else {
                FOOTSTEP_INTERVAL_TICKS
            });
            let Some(sound) = ground.sound(BlockSoundAction::Step) else {
                continue;
            };
            // Fire-and-forget at the FEET, so a remote's steps arrive from
            // their body.
            let handle = sounds.alloc_handle();
            self.audio.play_spatial_randomized(
                handle,
                sound,
                SpatialSoundSource::Fixed(step.pos),
                listener,
                step.pos,
            );
        }
        sounds
            .footstep_next_tick
            .retain(|id, _| footsteps.iter().any(|s| s.id == *id));
    }

    /// Idle cadence per live mob, on the live world clock.
    pub(super) fn tick_idle_mob_sounds(
        &mut self,
        sounds: &mut SessionSounds,
        listener: SpatialListener,
        mobs: &[MobPresentation],
        current_tick: u64,
    ) {
        for mob in mobs {
            if mob.dead {
                continue;
            }
            let Some(spec) = petramond::mob::def(mob.kind).sound_for(MobSoundCategory::Idle) else {
                continue;
            };
            let state = sounds
                .mob_states
                .entry(mob.id)
                .or_insert_with(|| MobSoundState {
                    next_idle_tick: current_tick.saturating_add(idle_delay_ticks(mob.id, 0, spec)),
                    sequence: 0,
                });
            if current_tick < state.next_idle_tick {
                continue;
            }
            state.sequence = state.sequence.wrapping_add(1);
            state.next_idle_tick =
                current_tick.saturating_add(idle_delay_ticks(mob.id, state.sequence, spec));
            let initial = sounds.mob_position(mob.id).unwrap_or(mob.pos);
            self.play_mob_sound(sounds, spec.sound, mob.id, listener, initial);
        }
        sounds
            .mob_states
            .retain(|id, _| mobs.iter().any(|m| m.id == *id && !m.dead));
    }

    fn play_mob_sound(
        &mut self,
        sounds: &mut SessionSounds,
        sound: Sound,
        mob_id: u64,
        listener: SpatialListener,
        initial: WorldPos,
    ) {
        let handle = sounds.alloc_handle();
        self.audio.play_spatial_randomized(
            handle,
            sound,
            SpatialSoundSource::Mob(mob_id),
            listener,
            initial,
        );
    }

    /// Everything played non-positionally since the last call.
    #[cfg(test)]
    pub(super) fn take_played_for_test(&mut self) -> Vec<Sound> {
        self.audio.take_played_for_test()
    }
}

impl SessionSounds {
    /// Record this render's interpolated mob positions.
    fn set_mob_positions(&mut self, mobs: &[MobPresentation], tick_alpha: f32) {
        self.mob_positions.clear();
        self.mob_positions.extend(
            mobs.iter()
                .map(|m| (m.id, m.prev_pos.lerp(m.pos, tick_alpha))),
        );
    }

    fn alloc_handle(&mut self) -> u64 {
        let handle = self.next_handle.max(MOB_SOUND_HANDLE_START);
        self.next_handle = handle.wrapping_add(1).max(MOB_SOUND_HANDLE_START);
        handle
    }

    fn mob_position(&self, mob_id: u64) -> Option<WorldPos> {
        self.mob_positions
            .iter()
            .find(|(id, _)| *id == mob_id)
            .map(|(_, pos)| *pos)
    }

    #[cfg(test)]
    pub(super) fn world_cue_count(&self) -> usize {
        self.world_cues.len()
    }

    /// The next spatial handle — every client-local spatial play allocates
    /// exactly one, so tests count plays by it.
    #[cfg(test)]
    pub(super) fn next_handle(&self) -> u64 {
        self.next_handle
    }

    #[cfg(test)]
    pub(super) fn mob_states_mut(&mut self) -> &mut HashMap<u64, MobSoundState> {
        &mut self.mob_states
    }

    #[cfg(test)]
    pub(super) fn footstep_tracks(&self) -> &HashMap<u64, u64> {
        &self.footstep_next_tick
    }

    #[cfg(test)]
    pub(super) fn set_mob_positions_for_test(&mut self, positions: Vec<(u64, WorldPos)>) {
        self.mob_positions = positions;
    }
}

/// The frame facts [`ClientAudio::render_world`] reads.
pub(super) struct WorldAudioFrame<'a> {
    pub listener: SpatialListener,
    pub mobs: &'a [MobPresentation],
    pub tick_alpha: f32,
    pub footsteps: &'a [FootstepSource],
    pub current_tick: u64,
}

/// The positional cue a world event is heard as, if any.
fn world_cue(ev: &WorldEvent) -> Option<(Sound, WorldPos)> {
    match *ev {
        WorldEvent::BlockPlaced { pos, block } => block
            .sound(BlockSoundAction::Place)
            .map(|s| (s, cell_centre(pos))),
        WorldEvent::BlockBroken { pos, block, .. } => block
            .sound(BlockSoundAction::Break)
            .map(|s| (s, cell_centre(pos))),
        WorldEvent::PanelToggled { anchor, open } => Some((
            if open {
                Sound::DoorOpen
            } else {
                Sound::DoorClose
            },
            cell_centre(anchor),
        )),
        WorldEvent::ChestOpened { pos } => Some((Sound::ChestOpen, cell_centre(pos))),
        WorldEvent::ChestClosed { pos } => Some((Sound::ChestClose, cell_centre(pos))),
        // The local player's own pickup keeps its non-positional sound
        // (`events.picked_up_item`); other players' pickups are heard at
        // their body.
        WorldEvent::ItemPickedUp { pos, by_self } => (!by_self).then_some((Sound::ItemPickup, pos)),
        // Particles only. A burst's SOUND is the producer's business through
        // the ordinary sound channel.
        WorldEvent::EmitterBurst { .. } => None,
    }
}

/// A cell's audible centre.
fn cell_centre(pos: IVec3) -> WorldPos {
    WorldPos::block_center(pos)
}

fn idle_delay_ticks(mob_id: u64, sequence: u64, spec: &petramond::mob::MobSoundSpec) -> u64 {
    let base = spec.tick_interval.unwrap_or(1) as u64;
    let variance = spec.tick_interval_variance as u64;
    let lo = base.saturating_sub(variance).max(1);
    let hi = base.saturating_add(variance).max(lo);
    lo + mix64(mob_id ^ sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15)) % (hi - lo + 1)
}

fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}
