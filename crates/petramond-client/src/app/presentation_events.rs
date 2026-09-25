use super::App;
use crate::game::{GameEvents, WorldEvent};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::{Block, BlockSoundAction};
use petramond_world::sound_registry::Sound;

impl App {
    pub(super) fn play_game_event_sounds(
        &mut self,
        events: &GameEvents,
        mining_block: Option<Block>,
        now: f64,
    ) {
        let mining_sound = mining_block.and_then(|b| b.sound(BlockSoundAction::Dig));
        self.audio.set_loop(mining_sound, now);

        // Mod-emitted sounds (the non-lossy tick queue): each plays once,
        // attenuated by distance to the player when positional.
        let listener = self.game.as_ref().map(|g| g.listener_position());
        for s in &events.sounds {
            let gain = match (s.pos, listener) {
                (Some(pos), Some(ear)) => positional_sound_gain(s.sound, pos, ear),
                _ => 1.0,
            };
            self.audio.play_attenuated(s.sound, gain);
        }
        self.spatial_sound_commands
            .extend(events.spatial_sounds.iter().copied());
        self.mob_sound_events
            .extend(events.mob_sounds.iter().copied());

        // World-anchored one-shots play POSITIONALLY, from the replicated
        // events (every observer hears them at the event's place — including
        // the local player's own actions; their old non-positional plays off
        // the `GameEvents` one-shots are gone). Buffered like the mob sounds
        // and started next render, where the spatial listener exists.
        for ev in &events.world_events {
            let cue = match *ev {
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
                // The local player's own pickup keeps its non-positional
                // sound (`events.picked_up_item` below); other players'
                // pickups are heard at their body.
                WorldEvent::ItemPickedUp { pos, by_self } => {
                    (!by_self).then_some((Sound::ItemPickup, pos))
                }
                // Particles only. A burst's SOUND is the producer's business
                // through the ordinary sound channel (a fluid splash emits
                // its small/big sound alongside the burst — see
                // `ServerGame::push_fluid_splash`).
                WorldEvent::EmitterBurst { .. } => None,
            };
            if let Some(cue) = cue {
                self.world_sound_cues.push(cue);
            }
        }

        if events.picked_up_item {
            self.audio.play(Sound::ItemPickup);
        }

        // Player damage: the hurt bark plus the subtle screen/hand shake the
        // next renders decay (see `App::render`).
        if events.player_damaged {
            self.audio.play(Sound::PlayerHurt);
            self.hurt_shake_t = super::HURT_SHAKE_SECS;
        }
    }

    pub(super) fn latch_game_event_hand_triggers(&mut self, events: &GameEvents) {
        if events.bed_interacted {
            self.sleep_interact_hand_t = super::SLEEP_INTERACT_HAND_SECS;
        }

        // The engine's gestures resolve to every local rig's graph events —
        // the viewmodel's and the body's — through the same lane a mod's
        // fired events arrive on.
        for (hand, kind) in events.one_shots() {
            self.hand_events
                .extend(petramond::player::one_shot::fired(hand, kind));
        }
        self.hand_events.extend_from_slice(&events.animator_events);
    }
}

fn positional_sound_gain(sound: Sound, pos: WorldPos, ear: WorldPos) -> f32 {
    let dist = (pos - ear).length();
    sound.distance_gain(dist)
}

/// A cell's audible centre.
fn cell_centre(pos: IVec3) -> WorldPos {
    WorldPos::block_center(pos)
}
