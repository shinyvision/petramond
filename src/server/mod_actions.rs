//! The drain for engine actions mod HostCalls queue mid-dispatch
//! ([`ModAction`]): a guest call arrives while the event bus is borrowed, so
//! calls that must run through a bus funnel (`DamagePlayer`, `DamageMob`) queue
//! here and `ServerGame` applies them at its per-tick action points (after every
//! systems batch and before each post-event drain — see `server::game`), on the
//! same tick, in queue order.

use crate::events::{DamageSource, ModAction};

use super::game::ServerGame;
use crate::game::tick::TickEvents;

impl ServerGame {
    /// Apply every queued mod action through the engine's own funnels, so the
    /// registered pre handlers (i-frames, hurt tuning) see them exactly like
    /// engine-originated damage. Actions queued *while* this batch runs (e.g.
    /// by a `player_damage_pre` handler) land at the next action point — the
    /// per-tick point count bounds them, no recursion.
    pub(crate) fn apply_mod_actions(&mut self, events: &mut TickEvents) {
        if !self.bus.queue_mut().has_actions() {
            return;
        }
        // The mod ABI is single-player-shaped: player-directed actions target
        // the HOST session (0) until per-player ABI addressing exists.
        let s = 0;
        for action in self.bus.queue_mut().take_actions() {
            match action {
                ModAction::DamagePlayer { amount, mod_id } => {
                    self.damage_player(s, amount, DamageSource::Mod(mod_id), None, events);
                }
                ModAction::KillPlayer { mod_id } => {
                    // Damage = current health, through the same funnel: a
                    // cancelling handler still saves the player.
                    let amount = self.sessions[s].player.health();
                    self.damage_player(s, amount, DamageSource::Mod(mod_id), None, events);
                }
                ModAction::DamageMob {
                    index,
                    amount,
                    mod_id,
                    origin,
                } => {
                    self.damage_mob_through_pipeline(
                        s,
                        index,
                        amount,
                        DamageSource::Mod(mod_id),
                        origin,
                        events,
                    );
                }
                // GUI opens share the ordered menu boundary with player
                // clicks and closes; this action point precedes that stage.
                ModAction::OpenGui { kind } => {
                    self.sessions[s].pending_menu_actions.push(
                        crate::server::player::PendingMenuAction::OpenModGui { kind, pos: None },
                    );
                }
                ModAction::CloseGui => {
                    self.sessions[s].request_close_mod_gui = true;
                }
                ModAction::ChatSend { text, targets } => {
                    let targets = match targets {
                        None => crate::server::chat::ChatTargets::All,
                        Some(ids) => crate::server::chat::ChatTargets::Players(
                            ids.into_iter()
                                .map(crate::server::player::PlayerId)
                                .collect(),
                        ),
                    };
                    self.enqueue_authored_chat(&text, targets);
                }
            }
        }
    }
}
