use super::App;
use crate::audio::Sound;
use crate::block::{Block, BlockSoundAction};
use crate::game::GameEvents;

pub(super) struct GameEventPresentation {
    pub(super) acted: bool,
}

impl App {
    pub(super) fn play_game_event_sounds(
        &mut self,
        events: &GameEvents,
        mining_block: Option<Block>,
        now: f64,
    ) {
        let mining_sound = mining_block.and_then(|b| b.sound(BlockSoundAction::Dig));
        self.audio.set_loop(mining_sound, now);

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
    }

    pub(super) fn latch_game_event_hand_triggers(
        &mut self,
        events: &GameEvents,
    ) -> GameEventPresentation {
        let opened_interactable = opened_interactable(events);

        self.hand.broke |= events.broke_block.is_some();
        self.hand.placed |=
            events.placed_block.is_some() || events.threw_item || opened_interactable;
        self.hand.swung |= events.swung_hand;

        GameEventPresentation {
            acted: events.broke_block.is_some()
                || events.placed_block.is_some()
                || events.threw_item
                || events.swung_hand
                || opened_interactable,
        }
    }
}

fn opened_interactable(events: &GameEvents) -> bool {
    events.open_crafting_table
        || events.open_furnace.is_some()
        || events.open_chest.is_some()
        || events.open_furniture_workbench.is_some()
        || events.toggled_door
}
