use super::{IdRemap, Remap};
use crate::net::protocol::{
    BlockDelta, BurstTextureMsg, ClientToServer, ColumnPayload, EntityLane, ItemSlotWire,
    ItemStateRow, JoinData, LightPayload, MenuSyncMsg, MenuTargetWire, MobStateRow, PlayerAction,
    PlayerActionKind, PlayerStateRow, RowSet, SectionPayload, SectionStatesPayload, SelfEvents,
    SelfRestore, SelfState, ServerToClient, SpatialSoundMsg, TickSection, TickUpdate,
    WorldEventMsg,
};
use crate::player::{AnimatorClaims, PlayerId};

impl Remap for ServerToClient {
    fn remap(&mut self, map: &IdRemap) -> bool {
        match self {
            ServerToClient::SectionData(p) => p.remap(map),
            ServerToClient::ColumnData(c) => c.remap(map),
            ServerToClient::LightData(l) => l.remap(map),
            ServerToClient::Tick(t) => t.remap(map),
            ServerToClient::JoinAccept(j) => j.remap(map),
            ServerToClient::HelloAck { .. }
            | ServerToClient::HelloReject { .. }
            | ServerToClient::ModList { .. }
            | ServerToClient::JoinReject { .. }
            | ServerToClient::SectionUnload { .. }
            | ServerToClient::ColumnUnload { .. }
            | ServerToClient::SectionCached { .. }
            | ServerToClient::PlayerJoined { .. }
            | ServerToClient::PlayerLeft { .. }
            | ServerToClient::ChatLine(_)
            | ServerToClient::RecipesUnlocked { .. }
            | ServerToClient::ModsDisabled { .. }
            | ServerToClient::StreamBatchStart
            | ServerToClient::StreamBatchEnd { .. }
            | ServerToClient::ServerClosing
            | ServerToClient::KeepAlive
            | ServerToClient::Disconnect { .. } => true,
        }
    }
}

pub(super) fn remap_to_server(map: &IdRemap, msg: &mut ClientToServer) {
    match msg {
        ClientToServer::Action(action) => match action {
            PlayerAction::BreakFinished {
                request_id: _,
                pos: _,
                tool_item_id,
                predicted: _,
            } => *tool_item_id = tool_item_id.and_then(|id| map.item_to_server(id)),
            PlayerAction::UseClick { .. }
            | PlayerAction::AttackClick { .. }
            | PlayerAction::Drop { .. }
            | PlayerAction::ThrowCursor { .. }
            | PlayerAction::Wake
            | PlayerAction::Respawn
            | PlayerAction::ToggleMode
            | PlayerAction::ToggleCreative
            | PlayerAction::ToggleFlight
            | PlayerAction::Creative(_)
            | PlayerAction::Schematic(_)
            | PlayerAction::OpenInventory
            | PlayerAction::CloseMenu => {}
        },
        ClientToServer::Hello { .. }
        | ClientToServer::KeyExchange { .. }
        | ClientToServer::ModQuery
        | ClientToServer::Join { .. }
        | ClientToServer::SetViewDistance { .. }
        | ClientToServer::SetCraftFilter { .. }
        | ClientToServer::PlayerUpdate(_)
        | ClientToServer::CreativeCursor { .. }
        | ClientToServer::MenuClick { .. }
        | ClientToServer::MenuDrag { .. }
        | ClientToServer::MenuDrop { .. }
        | ClientToServer::MenuSwapOffHand { .. }
        | ClientToServer::CraftRecipe { .. }
        | ClientToServer::ChatSend { .. }
        | ClientToServer::StreamBatchAck { .. }
        | ClientToServer::SectionCacheMiss { .. }
        | ClientToServer::Pause(_)
        | ClientToServer::KeepAlive
        | ClientToServer::Disconnect => {}
    }
}

impl Remap for SectionPayload {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let SectionPayload {
            pos: _,
            blocks,
            metrics,
            fluid: _,
            skylight: _,
            blocklight: _,
            states,
        } = self;
        let cube = std::sync::Arc::make_mut(&mut blocks.0);
        for b in cube.iter_mut() {
            *b = map.block(*b);
        }
        *metrics = petramond_world::section::Section::metrics_from_blocks(&blocks.0);
        states.remap(map)
    }
}

impl Remap for SectionStatesPayload {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let SectionStatesPayload {
            cell_states,
            cell_kv: _,
            draws: _,
        } = self;
        for (_, state) in cell_states {
            state.remap_ids(|id| map.block(id));
        }
        true
    }
}

impl Remap for ColumnPayload {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let ColumnPayload {
            pos: _,
            biomes,
            mesh_biomes,
            surface_heightmap: _,
            sky_cover: _,
            summaries: _,
            deep_band_lo: _,
        } = self;
        for bytes in [biomes, mesh_biomes] {
            for b in std::sync::Arc::make_mut(&mut bytes.0).iter_mut() {
                *b = map.biome(*b);
            }
        }
        true
    }
}

impl Remap for LightPayload {
    fn remap(&mut self, _map: &IdRemap) -> bool {
        let LightPayload {
            pos: _,
            skylight: _,
            blocklight: _,
        } = self;
        true
    }
}

impl Remap for JoinData {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let JoinData {
            player_id: _,
            player_name: _,
            seed: _,
            clock: _,
            tables: _,
            self_restore,
            crafting_recipes: _,
            players: _,
            client_policy: _,
        } = self;
        self_restore.remap(map)
    }
}

impl Remap for SelfRestore {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let SelfRestore {
            transform: _,
            mode: _,
            health: _,
            bed_spawn: _,
            effects: _,
            inventory,
            active_slot: _,
            craft_craftable_only: _,
            unlocked_recipes: _,
        } = self;
        inventory.remap(map)
    }
}

impl Remap for TickUpdate {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let TickUpdate {
            tick: _,
            clock: _,
            sections,
        } = self;
        sections.remap(map)
    }
}

impl Remap for TickSection {
    fn remap(&mut self, map: &IdRemap) -> bool {
        match self {
            TickSection::BlockDeltas(deltas) => deltas.remap(map),
            TickSection::Mobs(lane) => lane.remap(map),
            TickSection::Items(lane) => lane.remap(map),
            TickSection::Players(lane) => lane.remap(map),
            TickSection::PlayerActions(actions) => actions.remap(map),
            TickSection::SelfState(state) => state.remap(map),
            TickSection::MenuSync(sync) => sync.remap(map),
            TickSection::Events(events) => events.remap(map),
            TickSection::SelfEvents(events) => events.remap(map),
            TickSection::Creative(_) | TickSection::Schematics(_) => true,
            TickSection::BlockDraws(_) | TickSection::CellKvDeltas(_) => true,
            TickSection::SleepTally(_)
            | TickSection::ActionOutcomes(_)
            | TickSection::Env(_)
            | TickSection::OpenChests(_) => true,
        }
    }
}

impl Remap for BlockDelta {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let BlockDelta {
            pos: _,
            block_id,
            fluid: _,
            state,
            cell_kv: _,
        } = self;
        *block_id = map.block(*block_id);
        if let Some(state) = state {
            state.remap_ids(|id| map.block(id));
        }
        true
    }
}

impl<R: Remap + Clone> Remap for RowSet<R> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        self.retain_mut(|row| row.remap(map));
        true
    }
}

impl<R: Remap + Clone, K> Remap for EntityLane<R, K> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let EntityLane {
            despawned: _,
            spawned,
            updated,
        } = self;
        spawned.remap(map);
        updated.remap(map)
    }
}

impl Remap for MobStateRow {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let MobStateRow {
            id: _,
            kind_id,
            pos: _,
            yaw: _,
            tilt: _,
            anim_time: _,
            moving: _,
            idle_anim: _,
            head_yaw: _,
            head_pitch: _,
            hurt_timer: _,
            dead: _,
            shorn: _,
            emitters,
            conditions,
            anims: _,
            ragdoll: _,
            dig: _,
            held,
            draw: _,
        } = self;
        if !IdRemap::rewrite(kind_id, |id| map.mob(id)) {
            return false;
        }
        emitters.retain_mut(|e| IdRemap::rewrite(e, |id| map.emitter(id)));
        remap_conditions(map, conditions);
        for h in held {
            map.optional_item(h);
        }
        true
    }
}

impl Remap for ItemStateRow {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let ItemStateRow {
            id: _,
            item_id,
            count: _,
            data: _,
            pos: _,
            spin: _,
            flight: _,
        } = self;
        IdRemap::rewrite(item_id, |id| map.item(id))
    }
}

impl Remap for PlayerStateRow {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let PlayerStateRow {
            conditions,
            id: _,
            transform: _,
            on_ground: _,
            sneaking: _,
            sleeping: _,
            sleep_yaw: _,
            alive: _,
            visible: _,
            held_item,
            held_data: _,
            off_hand_item,
            off_hand_data: _,
            mining: _,
            eating: _,
            eating_off_hand: _,
            held_pose_main: _,
            held_pose_off: _,
            held_display,
            bone_poses: _,
            animator,
            hurt_recent: _,
            snap: _,
            mount: _,
        } = self;
        remap_conditions(map, conditions);
        map.optional_item(held_item);
        map.optional_item(off_hand_item);
        for shown in held_display {
            map.optional_item(shown);
        }
        animator.remap(map)
    }
}

impl Remap for SelfState {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let SelfState {
            conditions,
            health: _,
            mode: _,
            effects,
            inventory_revision: _,
            inventory,
            eating: _,
            eating_off_hand: _,
            sleeping: _,
            sleep_bed: _,
            move_scale: _,
            fly_scale: _,
            denied_actions: _,
            held_pose_main: _,
            held_pose_off: _,
            held_display,
            bone_poses: _,
            animator,
            transform: _,
        } = self;
        remap_conditions(map, conditions);
        effects.retain_mut(|(id, _)| IdRemap::rewrite(id, |e| map.effect(e)));
        inventory.remap(map);
        for shown in held_display {
            map.optional_item(shown);
        }
        animator.remap(map)
    }
}

impl Remap for ItemSlotWire {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let ItemSlotWire {
            item_id,
            count: _,
            data: _,
        } = self;
        IdRemap::rewrite(item_id, |id| map.item(id))
    }
}

impl Remap for AnimatorClaims {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let AnimatorClaims { params, plays } = self;
        params.retain_mut(|p| {
            let Some((rig, lut)) = map.animator(p.rig) else {
                return false;
            };
            match super::lookup(&lut.params, p.param as usize) {
                Some(local) => {
                    p.rig = rig;
                    p.param = local;
                    true
                }
                None => false,
            }
        });
        plays.retain_mut(|p| {
            let Some((rig, lut)) = map.animator(p.rig) else {
                return false;
            };
            match (
                super::lookup(&lut.slots, p.slot as usize),
                super::lookup(&lut.clips, p.clip as usize),
            ) {
                (Some(slot), Some(clip)) => {
                    p.rig = rig;
                    p.slot = slot;
                    p.clip = clip;
                    true
                }
                _ => false,
            }
        });
        true
    }
}

impl Remap for (PlayerId, PlayerActionKind) {
    fn remap(&mut self, map: &IdRemap) -> bool {
        match &mut self.1 {
            PlayerActionKind::Animator { rig, event } => map.remap_animator_event(rig, event),
            PlayerActionKind::Died | PlayerActionKind::Respawned => true,
        }
    }
}

impl Remap for WorldEventMsg {
    fn remap(&mut self, map: &IdRemap) -> bool {
        match self {
            WorldEventMsg::BlockBroken {
                pos: _,
                block_id,
                normal: _,
                tint: _,
            }
            | WorldEventMsg::BlockPlaced { pos: _, block_id } => {
                *block_id = map.block(*block_id);
                true
            }
            WorldEventMsg::PanelToggled { .. }
            | WorldEventMsg::ChestOpened { .. }
            | WorldEventMsg::ChestClosed { .. }
            | WorldEventMsg::ItemPickedUp { .. } => true,
            WorldEventMsg::MobSound {
                mob_id: _,
                kind_id,
                category: _,
                pos: _,
            } => IdRemap::rewrite(kind_id, |id| map.mob(id)),
            WorldEventMsg::Sound { sound_id, pos: _ } => {
                IdRemap::rewrite(sound_id, |id| map.sound(id))
            }
            WorldEventMsg::EmitterBurst {
                emitter_id,
                pos: _,
                intensity: _,
                direction: _,
                texture,
            } => {
                texture.remap(map);
                IdRemap::rewrite(emitter_id, |id| map.emitter(id))
            }
            WorldEventMsg::SpatialSound(cmd) => cmd.remap(map),
        }
    }
}

impl Remap for BurstTextureMsg {
    fn remap(&mut self, map: &IdRemap) -> bool {
        match self {
            BurstTextureMsg::Tile { .. } => {}
            BurstTextureMsg::Block { block_id, tint: _ } => *block_id = map.block(*block_id),
        }
        true
    }
}

impl Remap for SpatialSoundMsg {
    fn remap(&mut self, map: &IdRemap) -> bool {
        match self {
            SpatialSoundMsg::PlayAt { sound_id, .. }
            | SpatialSoundMsg::PlayOnMob { sound_id, .. } => {
                IdRemap::rewrite(sound_id, |id| map.sound(id))
            }
            SpatialSoundMsg::Set { .. } | SpatialSoundMsg::Stop { .. } => true,
        }
    }
}

impl Remap for SelfEvents {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let SelfEvents {
            picked_up_item: _,
            bed_interacted: _,
            player_damaged: _,
            player_died: _,
            sleep_ended: _,
            respawned: _,
            open_screen: _,
            close_document_gui: _,
            toggled_panel: _,
            used_unpredicted: _,
            used_unpredicted_off: _,
            animator_events,
            client_events: _,
        } = self;
        animator_events.retain_mut(|(rig, event)| map.remap_animator_event(rig, event));
        true
    }
}

impl Remap for MenuSyncMsg {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let MenuSyncMsg { target } = self;
        match target {
            MenuTargetWire::None => {}
            MenuTargetWire::Crafting { output } => {
                output.remap(map);
            }
            MenuTargetWire::Container {
                kind_key: _,
                anchor: _,
                slots,
                gui_state: _,
            } => {
                if let Some(slots) = slots {
                    slots.remap(map);
                }
            }
        }
        true
    }
}

fn remap_conditions(map: &IdRemap, conditions: &mut Vec<(u8, u8)>) {
    conditions.retain_mut(|(id, _)| IdRemap::rewrite(id, |c| map.condition(c)));
}
