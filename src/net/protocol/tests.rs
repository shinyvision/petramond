use std::sync::Arc;

use crate::mathh::{IVec3, Vec3};

use super::*;

fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
    let bytes = postcard::to_allocvec(v).expect("encode");
    let back: T = postcard::from_bytes(&bytes).expect("decode");
    assert_eq!(&back, v);
}

#[test]
fn representative_messages_roundtrip_through_postcard() {
    roundtrip(&ClientToServer::Hello { protocol: 1 });
    roundtrip(&ClientToServer::Join {
        player_name: "Rachel".into(),
        view_distance: 16,
        cached_sections: vec![SectionCacheClaim {
            pos: SectionPos::new(-3, 2, 40),
            hash: 0xDEAD_BEEF_u64,
        }],
    });
    roundtrip(&ClientToServer::SectionCacheMiss {
        pos: SectionPos::new(7, -1, 2),
    });
    roundtrip(&ServerToClient::SectionCached {
        pos: SectionPos::new(7, -1, 2),
        hash: 42,
    });
    roundtrip(&ServerToClient::SectionUnload {
        pos: SectionPos::new(1, 2, 3),
        cache_hash: Some(9),
    });
    roundtrip(&ServerToClient::ColumnUnload {
        pos: ChunkPos::new(5, -6),
        cache_hashes: vec![(0, 1), (3, u64::MAX)],
    });
    roundtrip(&ClientToServer::SetViewDistance { chunks: 24 });
    roundtrip(&ClientToServer::SetCraftFilter {
        craftable_only: true,
    });
    roundtrip(&ClientToServer::PlayerUpdate(PlayerUpdate {
        transform: Transform {
            pos: Vec3::new(1.5, 80.0, -3.25),
            vel: Vec3::ZERO,
            yaw: 1.25,
            pitch: -0.5,
        },
        on_ground: true,
        sneak: false,
        gameplay: true,
        break_held: true,
        use_held: false,
        target: Some(TargetRef {
            block: IVec3::new(4, 63, -2),
            normal: IVec3::new(0, 1, 0),
        }),
        hotbar_slot: 3,
        held_rotation: 1,
        wishdir: Vec3::ZERO,
        jump: false,
        sprint: false,
    }));
    roundtrip(&ClientToServer::Action(PlayerAction::UseClick {
        mob: Some(812),
        target: Some(TargetRef {
            block: IVec3::new(4, 65, -2),
            normal: IVec3::Y,
        }),
        request_id: Some(7),
        predicted: true,
        jabbed: false,
    }));
    roundtrip(&ClientToServer::Action(PlayerAction::AttackClick {
        mob: None,
        player: Some(2),
    }));
    roundtrip(&ClientToServer::MenuClick {
        slot: MenuSlotWire::Widget("kitchen:cook".into()),
        button: 0,
        shift: false,
        gather: true,
        request_id: 3,
    });
    roundtrip(&ClientToServer::MenuDrag {
        slots: vec![MenuSlotWire::Inventory(2), MenuSlotWire::Chest(4)],
        button: 1,
        request_id: 30,
    });
    roundtrip(&ClientToServer::MenuDrop {
        slot: MenuSlotWire::FurnaceOutput,
        all: true,
        request_id: 31,
    });
    roundtrip(&ClientToServer::CraftRecipe {
        recipe: "kitchen:bread".into(),
        bulk: true,
        request_id: 4,
    });
    roundtrip(&MenuSyncMsg {
        target: MenuTargetWire::Table {
            output: Some(ItemSlotWire {
                item_id: 7,
                count: 2,
            }),
        },
    });
    roundtrip(&ClientToServer::Action(PlayerAction::BreakFinished {
        request_id: 9,
        pos: IVec3::new(1, 2, 3),
        tool_item_id: None,
        predicted: true,
    }));
    roundtrip(&ClientToServer::ChatSend {
        text: "hello server".into(),
    });
    roundtrip(&ActionOutcome {
        id: 1,
        accepted: false,
        reason: Some(ActionDenyReason::TooFast),
    });
    roundtrip(&ServerToClient::ModList {
        mods: vec![ModEntry {
            id: "kitchen".into(),
            version: "0.1.0".into(),
        }],
    });
    roundtrip(&ServerToClient::ChatLine(ChatLine {
        seq: 9,
        spans: vec![
            ChatSpan {
                fg: ChatColor::Yellow,
                text: "Rachel".into(),
            },
            ChatSpan {
                fg: ChatColor::White,
                text: " joined".into(),
            },
        ],
    }));
    roundtrip(&ServerToClient::JoinReject {
        reason: JoinRejectReason::NameTaken,
    });
}

#[test]
fn arc_backed_section_payloads_roundtrip_byte_exact() {
    let blocks: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let payload = SectionPayload {
        pos: SectionPos {
            cx: -3,
            cy: 2,
            cz: 17,
        },
        blocks: SectionBytes(Arc::from(blocks.into_boxed_slice())),
        metrics: Default::default(),
        water: None,
        skylight: None,
        blocklight: None,
        states: SectionStatesPayload {
            doors: vec![(4095, 7)],
            slabs: vec![(9, [5, 3, 0])],
            model_cells: vec![(80, [1, 0, 1])],
            entity_facings: vec![(7, 2)],
            furnaces_lit: vec![7],
            cell_kv: vec![(12, vec![("kitchen:burn".into(), vec![1, 2, 3])])],
            ..Default::default()
        },
    };
    let bytes = postcard::to_allocvec(&ServerToClient::SectionData(Box::new(payload.clone())))
        .expect("encode");
    let back: ServerToClient = postcard::from_bytes(&bytes).expect("decode");
    let ServerToClient::SectionData(got) = back else {
        panic!("variant preserved");
    };
    assert_eq!(*got, payload);
    // The local path never serializes: cloning the message bumps the Arc.
    let cloned = payload.clone();
    assert!(Arc::ptr_eq(&cloned.blocks.0, &payload.blocks.0));
}

#[test]
fn tick_updates_roundtrip() {
    roundtrip(&ServerToClient::Tick(Box::new(TickUpdate {
        tick: 812,
        clock: 6_600,
        block_deltas: vec![
            BlockDelta {
                pos: IVec3::new(-8, 70, 3),
                block_id: 9,
                water: Some(0x87),
                state: None,
            },
            BlockDelta {
                pos: IVec3::new(4, 65, 4),
                block_id: 12,
                water: None,
                state: Some(CellState::Slab([1, 12, 0])),
            },
            BlockDelta {
                pos: IVec3::new(5, 65, 4),
                block_id: 30,
                water: None,
                state: Some(CellState::ModelCell {
                    off: [1, 0, 0],
                    facing: 3,
                }),
            },
        ],
        mobs: vec![MobStateRow {
            id: 4211,
            kind_id: 1,
            pos: Vec3::new(4.5, 71.0, -2.25),
            yaw: 0.75,
            anim_time: 12.5,
            moving: true,
            idle_anim: Some(1),
            head_yaw: -0.25,
            head_pitch: 0.1,
            hurt_timer: 0.2,
            dead: false,
            shorn: true,
            emitters: vec![1],
            anims: Vec::new(),
            ragdoll: Some(vec![([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0])]),
        }],
        items: vec![ItemStateRow {
            id: 7,
            item_id: 3,
            count: 12,
            pos: Vec3::new(0.5, 65.0, 0.5),
            spin: 1.25,
        }],
        players: vec![PlayerStateRow {
            id: PlayerId(1),
            transform: Transform {
                pos: Vec3::new(4.5, 71.0, -2.25),
                vel: Vec3::new(0.0, -0.5, 1.0),
                yaw: 0.75,
                pitch: -0.25,
            },
            on_ground: true,
            sneaking: false,
            sleeping: true,
            sleep_yaw: Some(1.5),
            alive: true,
            visible: true,
            held_item: Some(5),
            mining: Some((IVec3::new(4, 70, -2), 6)),
            eating: false,
            hurt_recent: true,
            snap: true,
            mount: None,
        }],
        player_actions: vec![
            (PlayerId(1), PlayerActionKind::Broke),
            (PlayerId(0), PlayerActionKind::AteFinished),
        ],
        self_state: Some(SelfState {
            health: 14,
            mode: 0,
            effects: vec![(0, 900)],
            inventory_revision: 42,
            inventory: Some(vec![
                Some(ItemSlotWire {
                    item_id: 5,
                    count: 64,
                }),
                None,
            ]),
            eating: Some(128),
            sleeping: None,
            sleep_bed: None,
            transform: Some(SelfTransform {
                transform: Transform {
                    pos: Vec3::new(1.5, 80.0, -3.25),
                    vel: Vec3::ZERO,
                    yaw: 1.25,
                    pitch: -0.5,
                },
                on_ground: true,
            }),
        }),
        open_chests: vec![IVec3::new(1, 65, 1)],
        env: Some(vec![
            ("petramond:time".into(), [0.5, 1.0, 3.0, 0.0]),
            ("petramond:light".into(), [1.0, 1.0, 1.0, 1.0]),
        ]),
        events: vec![
            WorldEventMsg::BlockBroken {
                pos: IVec3::new(4, 65, 4),
                block_id: 12,
                normal: Some(IVec3::Y),
            },
            WorldEventMsg::ItemPickedUp {
                pos: Vec3::new(1.0, 65.0, 2.0),
                by: PlayerId(1),
            },
            WorldEventMsg::ModSpatialSound(ModSpatialSoundMsg::PlayOnMob {
                handle: 3,
                sound_id: 2,
                mob_id: 4211,
                volume: 0.5,
                pitch: 1.1,
                last_pos: Vec3::new(0.0, 70.0, 0.0),
            }),
        ],
        self_events: SelfEvents {
            picked_up_item: true,
            open_screen: Some(OpenScreen::ModGui {
                kind_key: "kitchen:oven".into(),
                pos: Some(IVec3::new(4, 65, 4)),
            }),
            ..Default::default()
        },
        action_outcomes: vec![ActionOutcome {
            id: 1,
            accepted: true,
            reason: None,
        }],
        menu_sync: Some(MenuSyncMsg {
            target: MenuTargetWire::ModGui {
                kind_key: "kitchen:oven".into(),
                pos: Some(IVec3::new(4, 65, 4)),
                slots: Some(vec![
                    Some(ItemSlotWire {
                        item_id: 5,
                        count: 3,
                    }),
                    None,
                ]),
                gui_state: Some(vec![("kitchen:burn01".into(), GuiValueWire::F32(0.5))]),
            },
        }),
    })));
}
