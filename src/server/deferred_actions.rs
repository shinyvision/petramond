use crate::events::DeferredAction;

use super::game::ServerGame;
use crate::events::tick::TickEvents;

impl ServerGame {
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
                        continue;
                    };
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
                    self.damage_mob_through_pipeline(
                        mob_id, amount, source, origin, feedback, events,
                    );
                }
                DeferredAction::OpenGui {
                    player,
                    kind,
                    anchor,
                } => {
                    let Some(s) = self.sessions.index_of(player) else {
                        continue;
                    };
                    self.queue_menu_action(
                        s,
                        crate::server::player::PendingMenuAction::OpenGui { kind, anchor },
                    );
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
                DeferredAction::ContainerHold { mob_id, pos, open } => self
                    .containers
                    .set_mob_hold(&self.world, mob_id, pos, open, events),
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
