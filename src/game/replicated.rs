//! Client-side REPLICATED entity/self stores (multiplayer Phase C2c-i).
//!
//! The client renders mobs, dropped items, and its own HUD state from these
//! stores, fed by the per-tick [`TickUpdate`] batches the server emits — the
//! sim itself is unreachable (its own thread since Phase D). Locally the
//! batches are plain values over channels; over TCP (Phase E) the identical
//! messages arrive remapped, so nothing here changes.
//!
//! Each store keeps the PREVIOUS and CURRENT batch row per stable id — the
//! interpolation-ready pair `collect_mobs`/`collect_item_entities` blend at
//! `tick_alpha`, exactly as the renderer used to blend `Instance::prev_*`.
//! Light is deliberately absent from the rows: the client samples it at the
//! entity position from its REPLICA world (C2c-ii).

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Quat, Vec3};

use crate::crafting::CraftGrid;
use crate::gui::{ChestView, ContainerView, FurnaceView, GuiStateMap, WorkbenchView};
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::IVec3;
use crate::net::protocol::{
    ItemSlotWire, ItemStateRow, MenuSyncMsg, MenuTargetWire, MobStateRow, ModSpatialSoundMsg,
    SelfState, TickUpdate, WorldEventMsg,
};
use crate::player::{Player, PlayerMode};

use super::tick::WorldEvent;
use super::Game;

/// One replicated mob: the previous and current batch rows, keyed by the
/// mob's stable id in [`ReplicatedMobs`].
pub(crate) struct ReplicatedMob {
    pub(crate) prev: MobStateRow,
    pub(crate) curr: MobStateRow,
}

/// The client's replicated mob set. `BTreeMap` so presentation iterates in a
/// deterministic (id) order.
#[derive(Default)]
pub(crate) struct ReplicatedMobs {
    rows: BTreeMap<u64, ReplicatedMob>,
}

impl ReplicatedMobs {
    /// Apply one batch: a known id shifts curr→prev and adopts the new row, a
    /// fresh id starts with prev == curr (no interpolation from nowhere), and
    /// an id absent from the batch is dropped (killed/despawned server-side).
    pub(crate) fn apply(&mut self, batch: Vec<MobStateRow>) {
        let mut old = std::mem::take(&mut self.rows);
        for row in batch {
            let prev = match old.remove(&row.id) {
                Some(entry) => entry.curr,
                None => row.clone(),
            };
            self.rows.insert(row.id, ReplicatedMob { prev, curr: row });
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ReplicatedMob> {
        self.rows.values()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }
}

/// One replicated dropped item (prev/current batch rows).
pub(crate) struct ReplicatedItem {
    pub(crate) prev: ItemStateRow,
    pub(crate) curr: ItemStateRow,
}

/// The client's replicated dropped-item set — same contract as
/// [`ReplicatedMobs`].
#[derive(Default)]
pub(crate) struct ReplicatedItems {
    rows: BTreeMap<u64, ReplicatedItem>,
}

impl ReplicatedItems {
    pub(crate) fn apply(&mut self, batch: Vec<ItemStateRow>) {
        let mut old = std::mem::take(&mut self.rows);
        for row in batch {
            let prev = match old.remove(&row.id) {
                Some(entry) => entry.curr,
                None => row,
            };
            self.rows.insert(row.id, ReplicatedItem { prev, curr: row });
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ReplicatedItem> {
        self.rows.values()
    }
}

/// The client-side mirror of the local player's [`SelfState`]: everything the
/// HUD, hand, and overlays read. Seeded from the session at join (the wire
/// path seeds it from `SelfRestore`), then overwritten by every batch.
pub(crate) struct SelfView {
    /// Health in half-heart points.
    pub(crate) health: i32,
    pub(crate) mode: PlayerMode,
    /// Active effects (id, remaining ticks) in application order. Wire effect
    /// ids arrive already remapped to local ids, so they are stored directly.
    pub(crate) effects: Vec<(crate::effect::Effect, u32)>,
    /// A real `Inventory` value reconstructed from the wire slots — the menu
    /// renders slots + cursor from it. Contents refresh only when the server
    /// shipped them (revision moved); the active slot refreshes every batch.
    pub(crate) inventory: Inventory,
    /// The in-progress mining target + crack stage (0..=9).
    pub(crate) mining: Option<(IVec3, u8)>,
    /// The in-progress eat's progress in `[0, 1)`.
    pub(crate) eating: Option<f32>,
    /// The in-progress sleep's fade progress in `[0, 1]`.
    pub(crate) sleeping: Option<f32>,
    /// The in-progress sleep's bed base (foot) cell.
    pub(crate) sleep_bed: Option<IVec3>,
}

impl SelfView {
    /// Seed from the freshly-restored session player at world open — the
    /// in-process stand-in for the join handshake's `SelfRestore`, so the HUD
    /// is right on the very first frame (before any tick has run).
    pub(crate) fn seed_from(player: &Player) -> Self {
        Self {
            health: player.health(),
            mode: player.mode(),
            effects: player
                .effects()
                .iter()
                .map(|e| (e.effect, e.remaining))
                .collect(),
            inventory: player.inventory.clone(),
            mining: None,
            eating: None,
            sleeping: None,
            sleep_bed: None,
        }
    }

    /// Adopt one batch's self state.
    pub(crate) fn apply(&mut self, state: &SelfState) {
        self.health = state.health;
        self.mode = match state.mode {
            1 => PlayerMode::Spectator,
            _ => PlayerMode::Survival,
        };
        self.effects = state
            .effects
            .iter()
            .map(|&(id, remaining)| (crate::effect::Effect(id), remaining))
            .collect();
        if let Some(slots) = &state.inventory {
            self.inventory = inventory_from_wire(slots, state.active_slot);
        } else {
            self.inventory.set_active(state.active_slot);
        }
        self.mining = state.mining;
        self.eating = state.eating.map(|p| p as f32 / 255.0);
        self.sleeping = state.sleeping.map(|p| p as f32 / 255.0);
        self.sleep_bed = state.sleep_bed;
    }
}

/// The client's replicated MENU-session view, fed by [`MenuSyncMsg`]s (sent
/// on-change only) — the exclusive source `Game::menu_read_model` renders
/// from. Wire ids arrive already remapped to local ids.
pub(crate) struct MenuView {
    /// The open crafting grid (cells + the SERVER-computed result preview).
    pub(crate) craft: CraftGrid,
    pub(crate) furnace: Option<FurnaceView>,
    pub(crate) chest: Option<ChestView>,
    pub(crate) workbench: Option<WorkbenchView>,
    /// The open mod GUI's container slots.
    pub(crate) container: Option<ContainerView>,
    /// The open mod GUI's state map. Only replaced when a sync carries one
    /// (the server ships it on `Arc` change only).
    pub(crate) gui_state: Option<Arc<GuiStateMap>>,
}

impl Default for MenuView {
    fn default() -> Self {
        let mut craft = CraftGrid::new();
        craft.set_view(&[], None);
        Self {
            craft,
            furnace: None,
            chest: None,
            workbench: None,
            container: None,
            gui_state: None,
        }
    }
}

fn stack_from_wire(slot: Option<ItemSlotWire>) -> Option<ItemStack> {
    slot.map(|w| ItemStack::new(ItemType(w.item_id), w.count))
}

impl MenuView {
    /// Adopt one on-change sync: the target view is replaced whole; the mod
    /// GUI state map is kept unless the sync carries a fresh one.
    pub(crate) fn apply(&mut self, msg: MenuSyncMsg) {
        let cells: Vec<Option<ItemStack>> =
            msg.craft_grid.iter().map(|s| stack_from_wire(*s)).collect();
        self.craft
            .set_view(&cells, stack_from_wire(msg.craft_result));
        self.furnace = None;
        self.chest = None;
        self.workbench = None;
        self.container = None;
        match msg.target {
            MenuTargetWire::None | MenuTargetWire::Inventory | MenuTargetWire::Table => {
                self.gui_state = None;
            }
            MenuTargetWire::Furnace {
                slots,
                cook01,
                burn01,
                ..
            } => {
                self.gui_state = None;
                self.furnace = Some(FurnaceView {
                    input: stack_from_wire(slots[0]),
                    fuel: stack_from_wire(slots[1]),
                    output: stack_from_wire(slots[2]),
                    cook01,
                    burn01,
                });
            }
            MenuTargetWire::Chest { slots, .. } => {
                self.gui_state = None;
                let mut view = ChestView {
                    slots: [None; crate::world::chest::CHEST_SLOTS],
                };
                for (dst, src) in view.slots.iter_mut().zip(slots) {
                    *dst = stack_from_wire(src);
                }
                self.chest = Some(view);
            }
            MenuTargetWire::Workbench { input, results } => {
                self.gui_state = None;
                self.workbench = Some(WorkbenchView {
                    input: stack_from_wire(input),
                    results: results
                        .into_iter()
                        .map(|(id, ok)| (ItemType(id), ok))
                        .collect(),
                });
            }
            MenuTargetWire::ModGui {
                slots, gui_state, ..
            } => {
                self.container = slots.map(|slots| ContainerView {
                    slots: slots.into_iter().map(stack_from_wire).collect(),
                });
                if let Some(entries) = gui_state {
                    self.gui_state = Some(Arc::new(
                        entries
                            .into_iter()
                            .map(|(k, v)| (k, v.into_value()))
                            .collect(),
                    ));
                } else if self.gui_state.is_none() {
                    // First sight of this session without a map yet: render
                    // from the shared empty map until a change ships one.
                    self.gui_state = Some(crate::gui::empty_gui_state());
                }
            }
        }
    }
}

/// Rebuild a real [`Inventory`] from the wire layout (36 slots then the
/// cursor last — the `SelfRestore` layout). Short/absent tails read empty.
/// A real `Inventory` from wire slots (36 slots + cursor LAST — the
/// `SelfRestore`/`SelfState` layout). Also rebuilds the remote join's player.
pub(crate) fn inventory_from_wire(
    slots: &[Option<crate::net::protocol::ItemSlotWire>],
    active: u8,
) -> Inventory {
    let mut grid: [Option<ItemStack>; crate::inventory::TOTAL_SLOTS] =
        [None; crate::inventory::TOTAL_SLOTS];
    for (dst, src) in grid.iter_mut().zip(slots.iter()) {
        *dst = src.map(|w| ItemStack::new(crate::item::ItemType(w.item_id), w.count));
    }
    let cursor = slots
        .get(crate::inventory::TOTAL_SLOTS)
        .copied()
        .flatten()
        .map(|w| ItemStack::new(crate::item::ItemType(w.item_id), w.count));
    Inventory::from_parts(grid, cursor, active)
}

/// Interpolate a replicated ragdoll pose between two batches: positions lerp,
/// orientations slerp per bone. A fresh/mismatched previous pose (the ragdoll
/// just started, or a bone-count change) snaps to the current one.
pub(crate) fn lerp_ragdoll(
    prev: Option<&Vec<([f32; 3], [f32; 4])>>,
    curr: &[([f32; 3], [f32; 4])],
    alpha: f32,
) -> Vec<(Vec3, Quat)> {
    let to_pose = |&(p, q): &([f32; 3], [f32; 4])| (Vec3::from(p), Quat::from_array(q));
    match prev {
        Some(prev) if prev.len() == curr.len() => prev
            .iter()
            .zip(curr.iter())
            .map(|(a, b)| {
                let (pa, qa) = to_pose(a);
                let (pb, qb) = to_pose(b);
                (pa.lerp(pb, alpha), qa.slerp(qb, alpha))
            })
            .collect(),
        _ => curr.iter().map(to_pose).collect(),
    }
}

impl Game {
    /// Apply one pump's ordered server→client messages: terrain payloads into
    /// the REPLICA world, then the tick batch. A remote client applies the
    /// identical messages off the wire (remapped at its transport boundary).
    pub(crate) fn apply_server_messages(
        &mut self,
        msgs: &mut Vec<crate::net::protocol::ServerToClient>,
    ) {
        use crate::net::protocol::ServerToClient;
        for msg in msgs.drain(..) {
            match msg {
                ServerToClient::ColumnData(column) => self.replica.install_remote_column(column),
                ServerToClient::SectionData(section) => {
                    self.replica.install_remote_section(*section)
                }
                ServerToClient::LightData(light) => self.replica.install_remote_light(light),
                ServerToClient::SectionUnload(sp) => self.replica.uninstall_remote_section(sp),
                ServerToClient::ColumnUnload(cp) => self.replica.uninstall_remote_column(cp),
                ServerToClient::Tick(update) => self.apply_tick_update(update),
                // Roster changes (broadcast to every connection, local
                // included). The remote-player STORE keys off the per-tick
                // rows; the roster carries names (and survives even if a row
                // beats its PlayerJoined — the store refreshes names per
                // batch).
                ServerToClient::PlayerJoined { id, name } => {
                    self.player_roster.insert(id, name);
                }
                ServerToClient::PlayerLeft { id } => {
                    self.player_roster.remove(&id);
                }
                // Streaming flow control: Start opens the timing window, End
                // closes it into a measured apply rate and an immediate ack
                // (both markers apply in THIS same drain loop, so the elapsed
                // time is the real cost of installing the batch's messages).
                ServerToClient::StreamBatchStart => {
                    self.stream_batch_started = Some(std::time::Instant::now());
                }
                ServerToClient::StreamBatchEnd { count } => self.ack_stream_batch(count),
                ServerToClient::KeepAlive => {}
                ServerToClient::ServerClosing => {
                    self.note_connection_lost_because("the server closed");
                }
                ServerToClient::Disconnect { reason } => {
                    self.note_connection_lost_because(&format!("disconnected: {reason}"));
                }
                // Handshake messages never reach a joined session.
                other => {
                    debug_assert!(false, "unexpected post-join message: {other:?}");
                }
            }
        }
    }

    /// Close the open batch window into a rate sample and ack it RIGHT AWAY
    /// through the handle (not the frame outbox: acks must flow even on
    /// frames that never reach `tick_send`, or the server's window starves).
    /// The EMA smooths per-batch noise; the server clamps whatever we report.
    fn ack_stream_batch(&mut self, count: u32) {
        let Some(started) = self.stream_batch_started.take() else {
            return; // End without Start: tolerate, nothing to measure
        };
        let elapsed = started.elapsed().as_secs_f32().max(1e-4);
        let sampled = count as f32 / elapsed;
        let rate = match self.stream_rate_ema {
            Some(ema) => ema * 0.75 + sampled * 0.25,
            None => sampled,
        };
        self.stream_rate_ema = Some(rate);
        if self
            .handle
            .send(crate::net::protocol::ClientToServer::StreamBatchAck {
                messages_per_second: rate,
            })
            .is_err()
        {
            self.note_connection_lost();
        }
    }

    /// Adopt one replication batch: block deltas into the replica world, then
    /// the entity stores, self view, chest-lid set, the replicated clock, the
    /// menu-session view, and this window's events (translated to LOCAL types
    /// and buffered for the frame's `GameEvents`).
    pub(crate) fn apply_tick_update(&mut self, update: Box<TickUpdate>) {
        let update = *update;
        self.replicated_tick = update.tick;
        for delta in &update.block_deltas {
            self.replica.apply_remote_delta(*delta);
        }
        self.replicated_mobs.apply(update.mobs);
        self.replicated_items.apply(update.items);
        self.remote_players.apply(
            &update.players,
            &update.player_actions,
            self.self_id,
            &self.player_roster,
        );
        if let Some(state) = &update.self_state {
            self.self_view.apply(state);
            // Tick-side transform mutations (teleports, knockback) win over
            // the local prediction — per-field against what we last sent.
            if let Some(t) = &state.transform {
                self.adopt_authoritative_transform(t);
            }
        }
        // Shader-param environment (day/night sky, mod visuals): applied into
        // the REPLICA world's `WorldEnvironment` — the map the renderer reads
        // (`Game::environment` snapshots `replica.environment()` per frame).
        // `None` = unchanged since the last batch.
        if let Some(env) = update.env {
            for (key, value) in env {
                self.replica.set_shader_param(key, value);
            }
        }
        self.replica_clock.note_update();
        self.open_chests = update.open_chests.into_iter().collect();
        if let Some(sync) = update.menu_sync {
            self.menu_view.apply(sync);
        }
        for msg in update.events {
            self.buffer_world_event(msg);
        }
        self.pending_events
            .self_events
            .merge_from(update.self_events);
    }

    /// Translate one wire world event to local types into the frame buffer.
    /// Ids arrived remapped (identity in-process), so constructors are direct.
    fn buffer_world_event(&mut self, msg: WorldEventMsg) {
        use crate::game::tick::{MobSoundEvent, ModSound, ModSpatialSoundCommand};
        let ev = &mut self.pending_events;
        match msg {
            WorldEventMsg::BlockBroken {
                pos,
                block_id,
                normal,
            } => ev.world.push(WorldEvent::BlockBroken {
                pos,
                block: crate::block::Block::from_id(block_id),
                normal,
            }),
            WorldEventMsg::BlockPlaced { pos, block_id } => {
                ev.world.push(WorldEvent::BlockPlaced {
                    pos,
                    block: crate::block::Block::from_id(block_id),
                })
            }
            WorldEventMsg::DoorToggled { lower, open } => {
                ev.world.push(WorldEvent::DoorToggled { lower, open })
            }
            WorldEventMsg::ChestOpened { pos } => ev.world.push(WorldEvent::ChestOpened { pos }),
            WorldEventMsg::ChestClosed { pos } => ev.world.push(WorldEvent::ChestClosed { pos }),
            WorldEventMsg::ItemPickedUp { pos, by } => ev.world.push(WorldEvent::ItemPickedUp {
                pos,
                by_self: by == self.self_id,
            }),
            WorldEventMsg::MobSound {
                mob_id,
                kind_id,
                category,
                pos,
            } => ev.mob_sounds.push(MobSoundEvent {
                mob_id,
                kind: crate::mob::Mob(kind_id),
                category: crate::mob::MobSoundCategory::from_u8(category),
                pos,
            }),
            WorldEventMsg::ModSound { sound_id, pos } => ev.mod_sounds.push(ModSound {
                sound: crate::audio::Sound(sound_id),
                pos,
            }),
            WorldEventMsg::ModSpatialSound(cmd) => ev.mod_spatial_sounds.push(match cmd {
                ModSpatialSoundMsg::PlayAt {
                    handle,
                    sound_id,
                    pos,
                    volume,
                    pitch,
                } => ModSpatialSoundCommand::PlayAt {
                    handle,
                    sound: crate::audio::Sound(sound_id),
                    pos,
                    volume,
                    pitch,
                },
                ModSpatialSoundMsg::PlayOnMob {
                    handle,
                    sound_id,
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                } => ModSpatialSoundCommand::PlayOnMob {
                    handle,
                    sound: crate::audio::Sound(sound_id),
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                },
                ModSpatialSoundMsg::Stop { handle } => ModSpatialSoundCommand::Stop { handle },
            }),
        }
    }
}
