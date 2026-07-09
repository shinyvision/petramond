//! Registry id remapping at the TCP transport boundary.
//!
//! Dynamic block/item/mob/sound/effect ids are assigned per PROCESS at load,
//! so a client's ids need not match the server's (the client may have more
//! mods installed than the server enables). At join the server sends its name
//! tables in server-id order ([`NameTables`]); the client builds dense
//! server-id→client-id LUTs here and rewrites every inbound message right
//! after decode (and outbound before encode) on the transport threads.
//! Everything above the transport speaks client-local ids; the LOCAL
//! connection is identity and skips this module entirely.
//!
//! A server name unknown to the client can only be a server-side DISABLED
//! mod's registered residue (the handshake guarantees enabled mods are
//! installed): blocks map to air, items/mobs/sounds/effects to MISSING (the
//! consumer skips), each with one warning — the palette's unknown-name
//! semantics, never a rejection.

use super::protocol::{ClientToServer, NameTables, SectionBytes, ServerToClient};

/// LUT value for "the client doesn't know this name" in the non-block tables.
pub(crate) const MISSING: u16 = u16::MAX;

/// Dense server-id → client-id lookup tables.
#[derive(Debug)]
pub(crate) struct IdRemap {
    /// Blocks: unknown maps to air (0) — a cell must still hold SOMETHING.
    blocks: Vec<u8>,
    items: Vec<u16>,
    mobs: Vec<u16>,
    sounds: Vec<u16>,
    effects: Vec<u16>,
    /// True when every table is the identity — the fast path (a client whose
    /// registries happen to match the server's exactly).
    identity: bool,
}

impl IdRemap {
    /// Build the LUTs from the server's tables against THIS process's loaded
    /// registries.
    #[allow(dead_code)] // first production caller: the Phase E2 connect worker
    pub(crate) fn build(tables: &NameTables) -> IdRemap {
        let names = crate::registry::names();
        let blocks: Vec<u8> = tables
            .blocks
            .iter()
            .map(|n| match names.blocks.id(n) {
                Some(id) => id,
                None => {
                    log::warn!("remap: unknown server block '{n}' maps to air");
                    crate::block::Block::Air.0
                }
            })
            .collect();
        let items = build_u16(&tables.items, "item", |n| names.items.id(n));
        let mobs = build_u16(&tables.mobs, "mob", |n| {
            crate::mob::Mob::all()
                .iter()
                .find(|m| crate::mob::def(**m).key == n)
                .map(|m| m.0)
        });
        let sounds = build_u16(&tables.sounds, "sound", |n| {
            crate::audio::sound_by_name(n).map(|s| s.0)
        });
        let effects = build_u16(&tables.effects, "effect", |n| {
            crate::effect::by_name(n).map(|e| e.0)
        });

        let identity = blocks.iter().enumerate().all(|(i, &v)| i == v as usize)
            && [&items, &mobs, &sounds, &effects]
                .into_iter()
                .all(|t| t.iter().enumerate().all(|(i, &v)| i == v as usize));
        IdRemap {
            blocks,
            items,
            mobs,
            sounds,
            effects,
            identity,
        }
    }

    #[inline]
    #[allow(dead_code)] // the identity fast path reads the field; tests read this
    pub(crate) fn is_identity(&self) -> bool {
        self.identity
    }

    #[inline]
    pub(crate) fn block(&self, server_id: u8) -> u8 {
        self.blocks
            .get(server_id as usize)
            .copied()
            .unwrap_or(crate::block::Block::Air.0)
    }

    #[inline]
    pub(crate) fn item(&self, server_id: u8) -> Option<u8> {
        lut_u16(&self.items, server_id)
    }

    #[inline]
    pub(crate) fn mob(&self, server_id: u8) -> Option<u8> {
        lut_u16(&self.mobs, server_id)
    }

    #[inline]
    pub(crate) fn sound(&self, server_id: u8) -> Option<u8> {
        lut_u16(&self.sounds, server_id)
    }

    #[inline]
    pub(crate) fn effect(&self, server_id: u8) -> Option<u8> {
        lut_u16(&self.effects, server_id)
    }

    /// Rewrite a freshly-decoded server message to client-local ids, in place.
    /// EXHAUSTIVE over the enum: a new variant fails compilation here until
    /// its id story is decided (a `=> {}` arm is that decision, made visibly).
    pub(crate) fn remap_to_client(&self, msg: &mut ServerToClient) {
        if self.identity {
            return;
        }
        match msg {
            ServerToClient::SectionData(p) => {
                remap_bytes(&mut p.blocks, |id| self.block(id));
                // Slab records carry raw layer BLOCK IDS (the save codec's
                // 3-byte entry) — rewrite them like the block buffer.
                for (_, [_, a, b]) in &mut p.states.slabs {
                    *a = self.block(*a);
                    *b = self.block(*b);
                }
            }
            ServerToClient::Tick(t) => {
                for d in &mut t.block_deltas {
                    d.block_id = self.block(d.block_id);
                    if let Some(crate::net::protocol::CellState::Slab([_, a, b])) = &mut d.state {
                        *a = self.block(*a);
                        *b = self.block(*b);
                    }
                }
                // Unknown mob/item rows are DROPPED (skip semantics — a
                // disabled server-side mod's residue), like every non-block
                // unknown.
                t.mobs.retain_mut(|m| match self.mob(m.kind_id) {
                    Some(id) => {
                        m.kind_id = id;
                        true
                    }
                    None => false,
                });
                t.items.retain_mut(|i| match self.item(i.item_id) {
                    Some(id) => {
                        i.item_id = id;
                        true
                    }
                    None => false,
                });
                // Player rows: only the held item carries a registry id; an
                // unknown one reads as an empty hand (skip semantics — the
                // body itself always renders). `player_actions` kinds are
                // id-free, and `env` entries are param NAME strings + floats
                // — no registry ids ride either.
                for p in &mut t.players {
                    p.held_item = p.held_item.and_then(|id| self.item(id));
                }
                if let Some(s) = &mut t.self_state {
                    s.effects.retain_mut(|(id, _)| match self.effect(*id) {
                        Some(local) => {
                            *id = local;
                            true
                        }
                        None => false,
                    });
                    if let Some(slots) = &mut s.inventory {
                        for slot in slots {
                            remap_slot(self, slot);
                        }
                    }
                }
                // World events: block ids map to air (a cell-shaped fact);
                // unknown mob/sound events are DROPPED (skip semantics).
                // `self_events` carries no registry ids (the hand one-shots
                // are client-predicted, never echoed).
                t.events.retain_mut(|ev| self.remap_world_event(ev));
                if let Some(sync) = &mut t.menu_sync {
                    self.remap_menu_sync(sync);
                }
            }
            ServerToClient::JoinAccept(j) => {
                for slot in &mut j.self_restore.inventory {
                    if let Some(s) = slot {
                        match self.item(s.item_id) {
                            Some(id) => s.item_id = id,
                            None => *slot = None, // unknown item: slot reads empty
                        }
                    }
                }
                // Effects travel by name; tables ARE the vocabulary. Nothing
                // else in JoinData carries ids.
            }
            // Name-addressed or id-free messages:
            ServerToClient::HelloAck { .. }
            | ServerToClient::HelloReject { .. }
            | ServerToClient::ModList { .. }
            | ServerToClient::JoinReject { .. }
            | ServerToClient::ColumnData(_)
            | ServerToClient::LightData(_)
            | ServerToClient::SectionUnload(_)
            | ServerToClient::ColumnUnload(_)
            | ServerToClient::PlayerJoined { .. }
            | ServerToClient::PlayerLeft { .. }
            | ServerToClient::ChatLine(_)
            | ServerToClient::StreamBatchStart
            | ServerToClient::StreamBatchEnd { .. }
            | ServerToClient::ServerClosing
            | ServerToClient::KeepAlive
            | ServerToClient::Disconnect { .. } => {}
        }
    }

    /// Rewrite one world event's ids in place; `false` = drop the event (an
    /// unknown mob/sound — a disabled server-side mod's residue).
    fn remap_world_event(&self, ev: &mut super::protocol::WorldEventMsg) -> bool {
        use super::protocol::{ModSpatialSoundMsg, WorldEventMsg};
        match ev {
            WorldEventMsg::BlockBroken { block_id, .. }
            | WorldEventMsg::BlockPlaced { block_id, .. } => {
                *block_id = self.block(*block_id);
                true
            }
            WorldEventMsg::DoorToggled { .. }
            | WorldEventMsg::ChestOpened { .. }
            | WorldEventMsg::ChestClosed { .. }
            | WorldEventMsg::ItemPickedUp { .. } => true,
            WorldEventMsg::MobSound { kind_id, .. } => match self.mob(*kind_id) {
                Some(id) => {
                    *kind_id = id;
                    true
                }
                None => false,
            },
            WorldEventMsg::ModSound { sound_id, .. } => match self.sound(*sound_id) {
                Some(id) => {
                    *sound_id = id;
                    true
                }
                None => false,
            },
            WorldEventMsg::ModSpatialSound(cmd) => match cmd {
                ModSpatialSoundMsg::PlayAt { sound_id, .. }
                | ModSpatialSoundMsg::PlayOnMob { sound_id, .. } => match self.sound(*sound_id) {
                    Some(id) => {
                        *sound_id = id;
                        true
                    }
                    None => false,
                },
                // Stops carry no registry id and must reach the client so a
                // dropped-play's handle stays inert (stop of an unknown
                // handle is already a no-op).
                ModSpatialSoundMsg::Stop { .. } => true,
            },
        }
    }

    /// Rewrite a menu sync's item ids through the item LUT (unknown items
    /// read as empty slots / dropped workbench rows, the inventory policy).
    fn remap_menu_sync(&self, sync: &mut super::protocol::MenuSyncMsg) {
        use super::protocol::MenuTargetWire;
        for slot in &mut sync.craft_grid {
            remap_slot(self, slot);
        }
        remap_slot(self, &mut sync.craft_result);
        match &mut sync.target {
            MenuTargetWire::None | MenuTargetWire::Inventory | MenuTargetWire::Table => {}
            MenuTargetWire::Furnace { slots, .. } => {
                for slot in slots {
                    remap_slot(self, slot);
                }
            }
            MenuTargetWire::Chest { slots, .. } => {
                for slot in slots {
                    remap_slot(self, slot);
                }
            }
            MenuTargetWire::Workbench { input, results } => {
                remap_slot(self, input);
                results.retain_mut(|(id, _)| match self.item(*id) {
                    Some(local) => {
                        *id = local;
                        true
                    }
                    None => false,
                });
            }
            MenuTargetWire::ModGui { slots, .. } => {
                if let Some(slots) = slots {
                    for slot in slots {
                        remap_slot(self, slot);
                    }
                }
                // `gui_state` entries are mod-local strings — no registry ids.
            }
        }
    }

    /// Rewrite an outbound client message to server-local ids. No current
    /// client message carries registry ids; the exhaustive match makes a
    /// future one impossible to forget.
    pub(crate) fn remap_to_server(&self, msg: &mut ClientToServer) {
        if self.identity {
            return;
        }
        match msg {
            // MenuClick carries slot indices + widget-name strings, no
            // registry ids.
            ClientToServer::Hello { .. }
            | ClientToServer::ModQuery
            | ClientToServer::Join { .. }
            | ClientToServer::PlayerUpdate(_)
            | ClientToServer::Action(_)
            | ClientToServer::MenuClick { .. }
            | ClientToServer::ChatSend { .. }
            | ClientToServer::StreamBatchAck { .. }
            | ClientToServer::Pause(_)
            | ClientToServer::KeepAlive
            | ClientToServer::Disconnect => {}
        }
    }
}

/// THIS process's registry names, in id order — what a server sends as its
/// wire vocabulary at join.
pub(crate) fn local_name_tables() -> NameTables {
    let names = crate::registry::names();
    NameTables {
        blocks: (0..names.blocks.len())
            .map(|i| names.blocks.name(i as u8).expect("dense table").to_string())
            .collect(),
        items: (0..names.items.len())
            .map(|i| names.items.name(i as u8).expect("dense table").to_string())
            .collect(),
        mobs: crate::mob::Mob::all()
            .iter()
            .map(|m| crate::mob::def(*m).key.to_string())
            .collect(),
        sounds: crate::audio::sound_defs_for_net()
            .iter()
            .map(|d| d.name.to_string())
            .collect(),
        effects: crate::effect::Effect::all()
            .map(|e| e.def().name.to_string())
            .collect(),
    }
}

fn build_u16(server: &[String], what: &str, lookup: impl Fn(&str) -> Option<u8>) -> Vec<u16> {
    server
        .iter()
        .map(|n| match lookup(n) {
            Some(id) => id as u16,
            None => {
                log::warn!("remap: unknown server {what} '{n}' will be skipped");
                MISSING
            }
        })
        .collect()
}

#[inline]
fn lut_u16(table: &[u16], server_id: u8) -> Option<u8> {
    match table.get(server_id as usize).copied() {
        Some(MISSING) | None => None,
        Some(id) => Some(id as u8),
    }
}

/// Rewrite one item slot through the item LUT; unknown items read empty.
fn remap_slot(map: &IdRemap, slot: &mut Option<super::protocol::ItemSlotWire>) {
    if let Some(w) = slot {
        match map.item(w.item_id) {
            Some(id) => w.item_id = id,
            None => *slot = None,
        }
    }
}

/// Rewrite a section-sized id buffer in place. Decoded buffers are uniquely
/// owned, so this is a plain walk; a shared buffer (unexpected here) falls
/// back to copy-on-write.
fn remap_bytes(bytes: &mut SectionBytes, f: impl Fn(u8) -> u8) {
    let buf = std::sync::Arc::make_mut(&mut bytes.0);
    for b in buf.iter_mut() {
        *b = f(*b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mathh::IVec3;

    /// A "server" whose tables exactly match this process: identity.
    #[test]
    fn matching_registries_build_an_identity_remap() {
        let map = IdRemap::build(&local_name_tables());
        assert!(map.is_identity());
        assert_eq!(map.block(7), 7);
    }

    /// A server table naming something this client doesn't know (a disabled
    /// server-side mod's residue) degrades to air/skip with no rejection.
    #[test]
    fn unknown_server_names_map_to_air_or_missing() {
        let mut tables = local_name_tables();
        tables.blocks.push("ghost_mod:block".to_string());
        tables.items.push("ghost_mod:item".to_string());
        let unknown_block = (tables.blocks.len() - 1) as u8;
        let unknown_item = (tables.items.len() - 1) as u8;

        let map = IdRemap::build(&tables);
        assert!(!map.is_identity());
        assert_eq!(map.block(unknown_block), crate::block::Block::Air.0);
        assert_eq!(map.item(unknown_item), None);
        // Known ids still map through unchanged.
        assert_eq!(map.block(3), 3);
        assert_eq!(map.item(3), Some(3));
    }

    /// A permuted server table (same content, shifted ids — the realistic
    /// "client has extra mods" case in miniature) remaps buffers and deltas.
    #[test]
    fn shifted_server_ids_rewrite_sections_and_deltas() {
        let local = local_name_tables();
        // Server table = local names rotated by one: server id N = local id N+1.
        let mut tables = local.clone();
        tables.blocks.rotate_left(1);
        let map = IdRemap::build(&tables);
        assert!(!map.is_identity());

        let n = local.blocks.len() as u8;
        let mut msg = ServerToClient::Tick(Box::new(crate::net::protocol::TickUpdate {
            tick: 1,
            clock: 0,
            block_deltas: vec![
                crate::net::protocol::BlockDelta {
                    pos: IVec3::new(0, 64, 0),
                    block_id: 0, // server 0 = local 1 after the rotation
                    water: None,
                    state: None,
                },
                // The slab record's layer bytes are raw BLOCK IDS and must
                // rewrite like the id fields around them.
                crate::net::protocol::BlockDelta {
                    pos: IVec3::new(1, 64, 0),
                    block_id: 2,
                    water: None,
                    state: Some(crate::net::protocol::CellState::Slab([0b0111, 2, 3])),
                },
            ],
            ..Default::default()
        }));
        map.remap_to_client(&mut msg);
        let ServerToClient::Tick(t) = &msg else {
            unreachable!()
        };
        assert_eq!(t.block_deltas[0].block_id, 1 % n);
        assert_eq!(
            t.block_deltas[1].state,
            Some(crate::net::protocol::CellState::Slab([
                0b0111,
                3 % n,
                4 % n
            ])),
            "CellState::Slab layer ids rewrite through the block LUT"
        );

        let mut msg = ServerToClient::SectionData(Box::new(crate::net::protocol::SectionPayload {
            pos: crate::chunk::SectionPos {
                cx: 0,
                cy: 0,
                cz: 0,
            },
            blocks: SectionBytes(std::sync::Arc::from(vec![0u8, 1, 2].into_boxed_slice())),
            water: None,
            skylight: None,
            blocklight: None,
            states: crate::net::protocol::SectionStatesPayload {
                slabs: vec![(9, [0b0111, 2, 3])],
                ..Default::default()
            },
        }));
        map.remap_to_client(&mut msg);
        let ServerToClient::SectionData(p) = &msg else {
            unreachable!()
        };
        assert_eq!(&p.blocks.0[..], &[1 % n, 2 % n, 3 % n]);
        assert_eq!(
            p.states.slabs,
            vec![(9, [0b0111, 3 % n, 4 % n])],
            "SectionStatesPayload slab layer ids rewrite through the block LUT"
        );
    }

    /// Entity/self batches: known ids map through the mob/item/effect LUTs;
    /// unknown rows are DROPPED (skip semantics), and an unknown inventory
    /// item reads as an empty slot.
    #[test]
    fn tick_entity_batches_remap_known_ids_and_drop_unknown_rows() {
        use crate::net::protocol::{ItemSlotWire, ItemStateRow, MobStateRow, SelfState};
        let mut tables = local_name_tables();
        tables.mobs.push("ghost_mod:beast".to_string());
        tables.items.push("ghost_mod:trinket".to_string());
        tables.effects.push("ghost_mod:curse".to_string());
        let unknown_mob = (tables.mobs.len() - 1) as u8;
        let unknown_item = (tables.items.len() - 1) as u8;
        let unknown_effect = (tables.effects.len() - 1) as u8;
        let map = IdRemap::build(&tables);

        let mob_row = |kind_id: u8| MobStateRow {
            id: kind_id as u64,
            kind_id,
            pos: crate::mathh::Vec3::ZERO,
            yaw: 0.0,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            ragdoll: None,
        };
        let item_row = |item_id: u8| ItemStateRow {
            id: item_id as u64,
            item_id,
            count: 1,
            pos: crate::mathh::Vec3::ZERO,
            spin: 0.0,
        };
        let player_row = |held_item: Option<u8>| crate::net::protocol::PlayerStateRow {
            id: crate::server::player::PlayerId(1),
            pos: crate::mathh::Vec3::ZERO,
            vel: crate::mathh::Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            sneaking: false,
            sleeping: false,
            sleep_yaw: None,
            alive: true,
            visible: true,
            held_item,
            mining: None,
            eating: false,
            hurt_recent: false,
            snap: false,
        };
        let mut msg = ServerToClient::Tick(Box::new(crate::net::protocol::TickUpdate {
            mobs: vec![mob_row(0), mob_row(unknown_mob)],
            items: vec![item_row(2), item_row(unknown_item)],
            players: vec![player_row(Some(2)), player_row(Some(unknown_item))],
            self_state: Some(SelfState {
                health: 20,
                mode: 0,
                effects: vec![(0, 100), (unknown_effect, 50)],
                inventory_revision: 1,
                inventory: Some(vec![
                    Some(ItemSlotWire {
                        item_id: 2,
                        count: 4,
                    }),
                    Some(ItemSlotWire {
                        item_id: unknown_item,
                        count: 1,
                    }),
                ]),
                eating: None,
                sleeping: None,
                sleep_bed: None,
                transform: None,
            }),
            ..Default::default()
        }));
        map.remap_to_client(&mut msg);
        let ServerToClient::Tick(t) = &msg else {
            unreachable!()
        };
        assert_eq!(t.mobs.len(), 1, "the unknown mob row is dropped");
        assert_eq!(t.mobs[0].kind_id, 0);
        assert_eq!(t.items.len(), 1, "the unknown item row is dropped");
        assert_eq!(t.items[0].item_id, 2);
        assert_eq!(t.players.len(), 2, "player rows are never dropped");
        assert_eq!(t.players[0].held_item, Some(2));
        assert_eq!(
            t.players[1].held_item, None,
            "an unknown held item reads as an empty hand"
        );
        let s = t.self_state.as_ref().expect("self state kept");
        assert_eq!(s.effects, vec![(0, 100)], "the unknown effect is dropped");
        let slots = s.inventory.as_ref().expect("inventory kept");
        assert_eq!(slots[0].map(|w| w.item_id), Some(2));
        assert_eq!(slots[1], None, "an unknown inventory item reads empty");
    }
}
