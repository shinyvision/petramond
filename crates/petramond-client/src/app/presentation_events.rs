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
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let listener = Some(session.game.listener_position());
        self.sound
            .queue_game_events(&mut session.sounds, events, mining_block, listener, now);
        if events.player_damaged {
            session.hud_fx.latch_hurt();
        }
    }

    pub(super) fn latch_game_event_hand_triggers(&mut self, events: &GameEvents) {
        if let Some(session) = self.session.as_mut() {
            session.hud_fx.latch_hand_triggers(events);
        }
    }
}
