use std::sync::Arc;

use petramond_math::math::{IVec3, Vec3};

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
        slots: vec![MenuSlotWire::Inventory(2), MenuSlotWire::Container(4)],
        button: 1,
        request_id: 30,
    });
    roundtrip(&ClientToServer::MenuDrop {
        slot: MenuSlotWire::Container(2),
        all: true,
        request_id: 31,
    });
    roundtrip(&ClientToServer::CraftRecipe {
        recipe: "kitchen:bread".into(),
        bulk: true,
        request_id: 4,
    });
    roundtrip(&MenuSyncMsg {
        target: MenuTargetWire::Crafting {
            output: Some(ItemSlotWire {
                item_id: 7,
                count: 2,
                data: None,
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
    let blocks: Vec<u16> = (0..4096u32).map(|i| (i % 251) as u16).collect();
    // A COLOURED light cube: two bytes per cell, all three channels distinct,
    // so a lane slip or an endianness flip in `SectionLight` cannot pass.
    let light: Vec<petramond_world::light::LightRgb> = (0..4096u32)
        .map(|i| {
            petramond_world::light::LightRgb::new(
                (i % 31) as u8,
                (i / 7 % 31) as u8,
                (i / 53 % 31) as u8,
            )
        })
        .collect();
    let payload = SectionPayload {
        pos: SectionPos {
            cx: -3,
            cy: 2,
            cz: 17,
        },
        blocks: SectionBlocks(Arc::from(blocks.into_boxed_slice())),
        metrics: Default::default(),
        water: None,
        skylight: None,
        blocklight: Some(crate::net::protocol::SectionLight(Arc::from(
            light.into_boxed_slice(),
        ))),
        states: SectionStatesPayload {
            draws: Vec::new(),
            cell_states: vec![
                (4095, petramond_world::block::ShapeState::new(&[7])),
                (
                    9,
                    petramond_world::block::ShapeState::with_ids(&[5, 3, 0], 0b110),
                ),
                (80, petramond_world::block::ShapeState::new(&[1, 0, 1, 2])),
            ],
            cell_kv: vec![(12, vec![("kitchen:burn".into(), vec![1, 2, 3])])],
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
    assert!(Arc::ptr_eq(
        &cloned.blocklight.unwrap().0,
        &payload.blocklight.unwrap().0
    ));
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
                cell_kv: vec![("furniture:dye".into(), vec![200, 30, 40])],
            },
            BlockDelta {
                pos: IVec3::new(4, 65, 4),
                block_id: 12,
                water: None,
                state: Some(petramond_world::block::ShapeState::with_ids(
                    &[1, 12, 0],
                    0b110,
                )),
                cell_kv: vec![],
            },
            BlockDelta {
                pos: IVec3::new(5, 65, 4),
                block_id: 30,
                water: None,
                state: Some(petramond_world::block::ShapeState::new(&[1, 0, 0, 3])),
                cell_kv: vec![],
            },
        ],
        block_draws: vec![crate::net::protocol::BlockDrawDelta {
            pos: IVec3::new(1, 2, 3),
            prims: vec![mod_api::DrawPrim::Cuboid {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 0.5, 1.0],
                tile: "stone".into(),
                tint: [200, 120, 60],
                emissive: true,
            }]
            .into(),
        }],
        cell_kv_deltas: vec![
            CellKvDelta {
                pos: IVec3::new(4, 65, 4),
                key: "furniture:dye".into(),
                value: Some(vec![200, 30, 40]),
            },
            CellKvDelta {
                pos: IVec3::new(5, 65, 4),
                key: "farming:sips".into(),
                value: None,
            },
        ],
        mobs: vec![MobStateRow {
            id: 4211,
            kind_id: 1,
            pos: Vec3::new(4.5, 71.0, -2.25),
            yaw: 0.75,
            tilt: petramond_math::math::Tilt::LEVEL,
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
            data: None,
            pos: Vec3::new(0.5, 65.0, 0.5),
            spin: 1.25,
            flight: None,
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
            held_data: None,
            off_hand_item: Some(6),
            off_hand_data: None,
            mining: Some((IVec3::new(4, 70, -2), 6)),
            eating: false,
            eating_off_hand: false,
            held_pose_main: None,
            held_pose_off: None,
            held_display: [None; 2],
            // Non-empty on the ROW, because this is the field that ships for
            // every player every tick — an encoding that silently drops it
            // would look exactly like nobody posing anything.
            bone_poses: vec![crate::player::BonePose {
                bone: 3,
                rotation: [-11.0, 3.0, 41.0],
                translation: [0.0, 1.0, -2.0],
                hold: true,
            }],
            motion_claims: [
                crate::player::HandMotions::of([mod_api::HandMotion::Swing]),
                crate::player::HandMotions::NONE,
            ],
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
                    data: None,
                }),
                None,
            ]),
            eating: Some(128),
            eating_off_hand: true,
            move_scale: 0.5,
            denied_actions: crate::player::DeniedActions::of([mod_api::BodyAction::Mine]),
            held_pose_main: Some(mod_api::HeldPose {
                first_person: mod_api::HeldPoseData {
                    rotation: [0.0, 2.5, 0.0],
                    translation: [1.25, -3.5, -4.0],
                },
                third_person: mod_api::HeldPoseData::IDENTITY,
            }),
            held_pose_off: None,
            held_display: [None; 2],
            bone_poses: vec![crate::player::BonePose {
                bone: 7,
                rotation: [8.0, -2.0, -29.0],
                translation: [0.5, 0.0, 1.5],
                hold: false,
            }],
            motion_claims: [
                crate::player::HandMotions::NONE,
                crate::player::HandMotions::of([mod_api::HandMotion::Jab]),
            ],
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
                tint: None,
            },
            WorldEventMsg::ItemPickedUp {
                pos: Vec3::new(1.0, 65.0, 2.0),
                by: PlayerId(1),
            },
            WorldEventMsg::SpatialSound(SpatialSoundMsg::PlayOnMob {
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
            open_screen: Some(OpenScreen::Gui {
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
            target: MenuTargetWire::Container {
                kind_key: "kitchen:oven".into(),
                pos: Some(IVec3::new(4, 65, 4)),
                slots: Some(vec![
                    Some(ItemSlotWire {
                        item_id: 5,
                        count: 3,
                        data: None,
                    }),
                    None,
                ]),
                gui_state: Some(vec![("kitchen:burn01".into(), GuiValueWire::F32(0.5))]),
            },
        }),
    })));
}

/// The section payload's block cube is palette-packed on the wire, and the
/// only thing that can go wrong there is silent narrowing. A cube spanning
/// the byte boundary — and one past the NARROW palette index — must come back
/// cell-for-cell.
#[test]
fn wire_block_cubes_carry_ids_past_one_byte() {
    let cases: Vec<Vec<u16>> = vec![
        vec![0, 1, 255, 256, 257, 4095, 0, 256],
        // Every cell distinct, so the packer must take its wide-index arm.
        (0..600u16).collect(),
        // Uniform: the shortest possible palette.
        vec![777; 4096],
    ];
    for cells in cases {
        let payload = SectionPayload {
            pos: SectionPos {
                cx: 1,
                cy: -2,
                cz: 3,
            },
            blocks: SectionBlocks(Arc::from(cells.clone().into_boxed_slice())),
            metrics: Default::default(),
            water: None,
            skylight: None,
            blocklight: None,
            states: Default::default(),
        };
        let bytes = postcard::to_allocvec(&payload).expect("encode");
        let back: SectionPayload = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(&back.blocks.0[..], &cells[..], "{} cells", cells.len());
    }
}
