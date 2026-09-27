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

const MOB_SOUND_HANDLE_START: u64 = 1 << 63;

const FOOTSTEP_INTERVAL_TICKS: u64 = 10;
const SPRINT_FOOTSTEP_INTERVAL_TICKS: u64 = 7;

#[derive(Default)]
pub(super) struct MobSoundState {
    pub(super) next_idle_tick: u64,
    pub(super) sequence: u64,
}

pub(super) struct ClientAudio {
    audio: Audio,
    music: MusicDirector,
}

pub(super) struct SessionSounds {
    spatial_commands: Vec<SpatialSoundCommand>,
    mob_positions: Vec<(u64, WorldPos)>,
    mob_events: Vec<MobSoundEvent>,
    world_cues: Vec<(Sound, WorldPos)>,
    mob_states: HashMap<u64, MobSoundState>,
    footstep_next_tick: HashMap<u64, u64>,
    loop_gains: Vec<(Sound, f32)>,
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

    pub(super) fn play_interface(&mut self, sound: Sound) {
        self.audio.play_interface(sound);
    }

    pub(super) fn set_volumes(&mut self, master: f32, sound: f32, music: f32) {
        self.audio.set_volumes(master, sound, music);
    }

    pub(super) fn engine(&self) -> &Audio {
        &self.audio
    }

    pub(super) fn engine_mut(&mut self) -> &mut Audio {
        &mut self.audio
    }

    pub(super) fn clear_spatial(&mut self) {
        self.audio.clear_spatial();
    }

    pub(super) fn stop_mining_loop(&mut self, now: f64) {
        self.audio.set_loop(None, now);
    }

    pub(super) fn update_session(&mut self, in_session: bool, world_frozen: bool, dt: f32) {
        self.audio.set_spatial_paused(world_frozen);
        self.music
            .update(&mut self.audio, in_session, world_frozen, dt);
    }

    pub(super) fn end_session(&mut self, now: f64) {
        self.audio.set_loop(None, now);
        self.audio.stop_gain_loops();
        self.audio.clear_spatial();
    }

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

    #[cfg(test)]
    pub(super) fn take_played_for_test(&mut self) -> Vec<Sound> {
        self.audio.take_played_for_test()
    }
}

impl SessionSounds {
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

pub(super) struct WorldAudioFrame<'a> {
    pub listener: SpatialListener,
    pub mobs: &'a [MobPresentation],
    pub tick_alpha: f32,
    pub footsteps: &'a [FootstepSource],
    pub current_tick: u64,
}

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
        WorldEvent::ItemPickedUp { pos, by_self } => (!by_self).then_some((Sound::ItemPickup, pos)),
        WorldEvent::EmitterBurst { .. } => None,
    }
}

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
