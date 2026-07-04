//! The drain for engine actions mod HostCalls queue mid-dispatch
//! ([`ModAction`]): a guest call arrives while the event bus is borrowed, so
//! calls that must run through a bus funnel (`DamagePlayer`, `HurtMob`) queue
//! here and `Game` applies them at its per-tick action points (after every
//! systems batch and before each post-event drain — see `game::tick`), on the
//! same tick, in queue order.

use crate::events::{DamageSource, ModAction};

use super::tick::TickEvents;
use super::Game;

impl Game {
    /// Apply every queued mod action through the engine's own funnels, so the
    /// registered pre handlers (i-frames, hurt tuning) see them exactly like
    /// engine-originated damage. Actions queued *while* this batch runs (e.g.
    /// by a `player_damage_pre` handler) land at the next action point — the
    /// per-tick point count bounds them, no recursion.
    pub(super) fn apply_mod_actions(&mut self, events: &mut TickEvents) {
        if !self.bus.queue_mut().has_actions() {
            return;
        }
        for action in self.bus.queue_mut().take_actions() {
            match action {
                ModAction::DamagePlayer { amount, mod_id } => {
                    self.damage_player(amount, DamageSource::Mod(mod_id), events);
                }
                ModAction::KillPlayer { mod_id } => {
                    // Damage = current health, through the same funnel: a
                    // cancelling handler still saves the player.
                    let amount = self.player.health();
                    self.damage_player(amount, DamageSource::Mod(mod_id), events);
                }
                ModAction::HurtMob {
                    index,
                    amount,
                    from,
                } => {
                    self.hurt_mob_through_pipeline(index, amount, from, events);
                }
                // Screen requests ride the same one-shot fields a block
                // interact uses; the app shell applies them next frame.
                ModAction::OpenGui { kind } => {
                    self.request_open_mod_gui = Some((kind, None));
                }
                ModAction::CloseGui => {
                    self.request_close_mod_gui = true;
                }
            }
        }
    }
}
