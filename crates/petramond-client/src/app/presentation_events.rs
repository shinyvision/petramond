use super::App;
use crate::game::GameEvents;
use petramond_world::block::Block;

impl App {
    pub(super) fn play_game_event_sounds(
        &mut self,
        events: &GameEvents,
        mining_block: Option<Block>,
        now: f64,
    ) {
        let listener = self.game.as_ref().map(|g| g.listener_position());
        self.sound
            .queue_game_events(events, mining_block, listener, now);
        // Player damage: the subtle screen/hand shake the next renders decay
        // (see `App::render`); the hurt bark played with the events above.
        if events.player_damaged {
            self.hud_fx.latch_hurt();
        }
    }

    pub(super) fn latch_game_event_hand_triggers(&mut self, events: &GameEvents) {
        self.hud_fx.latch_hand_triggers(events);
    }
}
