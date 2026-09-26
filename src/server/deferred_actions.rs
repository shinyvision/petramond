//! The drain for engine actions mod HostCalls queue mid-dispatch
//! ([`DeferredAction`]): a guest call arrives while the event bus is borrowed, so
//! calls that must run through a bus funnel (`DamagePlayer`, `DamageMob`) queue
//! here and `ServerGame` applies them at its per-tick action points (after every
//! systems batch and before each post-event drain — see `server::game`), on the
//! same tick, in queue order.

use crate::events::DeferredAction;

use super::game::ServerGame;
use crate::events::tick::TickEvents;

impl ServerGame {
    /// Apply every queued mod action through the engine's own funnels, so
    /// global immunity and registered pre handlers treat them exactly like
    /// engine-originated damage. Actions queued *while* this batch runs (e.g.
    /// by a `player_damage_pre` handler) land at the next action point — the
    /// per-tick point count bounds them, no recursion.
    pub fn apply_deferred_actions(&mut self, events: &mut TickEvents) {
        if !self.mods.bus_mut().queue_mut().has_actions() {
            return;
        }
        for action in self.mods.bus_mut().queue_mut().take_actions() {
            match action {
                DeferredAction::DamagePlayer {
                    player,
                    amount,
                    source,
                    origin,
                } => {
                    let Some(t) = self.sessions.index_of(player) else {
                        continue; // the named session left before the drain
                    };
                    // A named attacker's hit is the engine's own melee in
                    // every consequence, the shove included.
                    if self.damage_player(t, amount, source, origin, events) && source.is_attack() {
                        if let Some(from) = origin {
                            let scale = self.weapon_knockback(source);
                            self.shove_player(t, from, scale);
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
                    // The pipeline acts for the attacker the source names (a
                    // `mob_damage_pre` handler then reads the same actor the
                    // engine's own hit shows it).
                    self.damage_mob_through_pipeline(
                        index, amount, source, origin, feedback, events,
                    );
                }
                // GUI opens share the ordered menu boundary with player
                // clicks and closes; this action point precedes that stage.
                DeferredAction::OpenGui {
                    player,
                    kind,
                    anchor,
                } => {
                    let Some(s) = self.sessions.index_of(player) else {
                        continue;
                    };
                    self.sessions[s]
                        .input.pending_menu_actions
                        .push(crate::server::player::PendingMenuAction::OpenGui { kind, anchor });
                }
                DeferredAction::CloseGui { player } => {
                    let Some(s) = self.sessions.index_of(player) else {
                        continue;
                    };
                    self.sessions[s].replication.request_close_gui = true;
                }
                DeferredAction::ActorBreak {
                    mob_id,
                    pos,
                    target,
                    tool_slot,
                    collect,
                } => self.apply_actor_break(mob_id, pos, target, tool_slot, collect, events),
                DeferredAction::ActorPlace {
                    mob_id,
                    pos,
                    record,
                    pay,
                } => self.apply_actor_place(mob_id, pos, record, pay, events),
                DeferredAction::ActorInteract { mob_id, pos } => {
                    self.apply_actor_interact(mob_id, pos, events)
                }
                DeferredAction::ContainerHold { mob_id, pos, open } => {
                    self.containers
                        .set_mob_hold(&self.world, mob_id, pos, open, events)
                }
                DeferredAction::SchematicChoose { player, tag } => {
                    self.open_schematic_choice(player, tag)
                }
                DeferredAction::SchematicPosition {
                    player,
                    tag,
                    asset,
                    origin,
                    turns,
                } => self.open_schematic_position(player, tag, asset, origin, turns),
                DeferredAction::ChatSend { text, targets } => {
                    let targets = match targets {
                        None => crate::server::chat::ChatTargets::All,
                        Some(ids) => crate::server::chat::ChatTargets::Players(
                            ids.into_iter().map(crate::player::PlayerId).collect(),
                        ),
                    };
                    self.chat.authored(&text, targets);
                }
            }
        }
    }
}
