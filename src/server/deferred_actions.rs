//! The drain for engine actions mod HostCalls queue mid-dispatch
//! ([`DeferredAction`]): a guest call arrives while the event bus is borrowed, so
//! calls that must run through a bus funnel (`DamagePlayer`, `DamageMob`) queue
//! here and `ServerGame` applies them at its per-tick action points (after every
//! systems batch and before each post-event drain — see `server::game`), on the
//! same tick, in queue order.

use crate::events::{DamageSource, DeferredAction};

use super::game::ServerGame;
use crate::events::tick::TickEvents;

impl ServerGame {
    /// Apply every queued mod action through the engine's own funnels, so
    /// global immunity and registered pre handlers treat them exactly like
    /// engine-originated damage. Actions queued *while* this batch runs (e.g.
    /// by a `player_damage_pre` handler) land at the next action point — the
    /// per-tick point count bounds them, no recursion.
    pub fn apply_deferred_actions(&mut self, events: &mut TickEvents) {
        if !self.bus.queue_mut().has_actions() {
            return;
        }
        // The GUI and chat actions below are single-player-shaped: they
        // target the HOST session (0) until per-player ABI addressing
        // reaches them.
        let s = 0;
        for action in self.bus.queue_mut().take_actions() {
            match action {
                DeferredAction::DamagePlayer {
                    player,
                    amount,
                    source,
                    origin,
                } => {
                    let Some(t) = self.sessions.iter().position(|sess| sess.id == player) else {
                        continue; // the named session left before the drain
                    };
                    // A named attacker's hit is the engine's own melee in
                    // every consequence, the shove included.
                    if self.damage_player(t, amount, source, origin, events) && source.is_attack() {
                        if let Some(from) = origin {
                            self.shove_player(t, from);
                        }
                    }
                }
                DeferredAction::DamageMob {
                    mob_id,
                    amount,
                    source,
                    origin,
                    feedback,
                } => {
                    // Resolve the STABLE id only now: earlier actions in this
                    // drain may have removed mobs and shifted indices. A mob
                    // gone by drain time is a silent no-op (the pipeline also
                    // rejects the dead).
                    let Some(index) = self.world.mobs().index_of_id(mob_id) else {
                        continue;
                    };
                    // The pipeline's acting session is the attacker's when
                    // the source names one (a `mob_damage_pre` handler then
                    // reads the same actor the engine's own hit shows it).
                    let acting = match source {
                        DamageSource::PlayerAttack(id) => self
                            .sessions
                            .iter()
                            .position(|sess| sess.id == id)
                            .unwrap_or(s),
                        _ => s,
                    };
                    self.damage_mob_through_pipeline(
                        acting, index, amount, source, origin, feedback, events,
                    );
                }
                // GUI opens share the ordered menu boundary with player
                // clicks and closes; this action point precedes that stage.
                DeferredAction::OpenGui { kind } => {
                    self.sessions[s].pending_menu_actions.push(
                        crate::server::player::PendingMenuAction::OpenGui { kind, pos: None },
                    );
                }
                DeferredAction::CloseGui => {
                    self.sessions[s].request_close_gui = true;
                }
                DeferredAction::ChatSend { text, targets } => {
                    let targets = match targets {
                        None => crate::server::chat::ChatTargets::All,
                        Some(ids) => crate::server::chat::ChatTargets::Players(
                            ids.into_iter().map(crate::player::PlayerId).collect(),
                        ),
                    };
                    self.enqueue_authored_chat(&text, targets);
                }
            }
        }
    }
}
