use super::App;
use crate::audio::Sound;
use crate::block::{Block, BlockSoundAction};
use crate::game::GameEvents;
use crate::mathh::Vec3;

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
        for s in &events.mod_sounds {
            let gain = match (s.pos, listener) {
                (Some(pos), Some(ear)) => mod_sound_gain(s.sound, pos, ear),
                _ => 1.0,
            };
            self.audio.play_attenuated(s.sound, gain);
        }
        self.spatial_sound_commands
            .extend(events.mod_spatial_sounds.iter().copied());

        if let Some(b) = events.placed_block {
            if let Some(s) = b.sound(BlockSoundAction::Place) {
                self.audio.play(s);
            }
        }

        if let Some(b) = events.broke_block {
            if let Some(s) = b.sound(BlockSoundAction::Break) {
                self.audio.play(s);
            }
        }

        if events.picked_up_item {
            self.audio.play(Sound::ItemPickup);
        }

        if let Some(now_open) = events.toggled_door {
            self.audio.play(if now_open {
                Sound::DoorOpen
            } else {
                Sound::DoorClose
            });
        }

        if events.open_chest.is_some() {
            self.audio.play(Sound::ChestOpen);
        }
    }

    pub(super) fn latch_game_event_hand_triggers(&mut self, events: &GameEvents) {
        let opened_interactable = opened_interactable(events);

        self.hand.broke |= events.broke_block.is_some();
        self.hand.placed |= events.placed_block.is_some()
            || events.threw_item
            || events.used_item
            || opened_interactable;
        self.hand.swung |= events.swung_hand;
    }
}

fn mod_sound_gain(sound: Sound, pos: Vec3, ear: Vec3) -> f32 {
    let dist = (pos - ear).length();
    sound.distance_gain(dist)
}

fn opened_interactable(events: &GameEvents) -> bool {
    events.open_crafting_table
        || events.open_furnace.is_some()
        || events.open_chest.is_some()
        || events.open_furniture_workbench.is_some()
        || events.toggled_door.is_some()
}
