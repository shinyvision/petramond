use std::sync::Arc;

use petramond_math::math::{IVec3, Vec3};

use super::*;
use petramond_math::world_pos::WorldPos;

fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
    let bytes = postcard::to_allocvec(v).expect("encode");
    let back: T = postcard::from_bytes(&bytes).expect("decode");
    assert_eq!(&back, v);
}

#[test]
fn representative_messages_roundtrip_through_postcard() {
    roundtrip(&ClientToServer::Action(PlayerAction::Creative(
        crate::schematic::CreativeAction::Redo,
    )));
    roundtrip(&ClientToServer::Hello { protocol: 1 });
    roundtrip(&ClientToServer::Join {
        credential: crate::net::protocol::JoinCredential::Name("Rachel".into()),
        key: crate::net::identity::PlayerKey([0xAB; 32]),
        proof: vec![0x5A; 64],
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
            pos: WorldPos::new(1.5, 80.0, -3.25),
            vel: Vec3::ZERO,
            yaw: 1.25,
            pitch: -0.5,
        },
        on_ground: true,
        sneak: false,
        gameplay: true,
        break_held: true,
        use_held: false,
        target: Some(TargetRef::face(IVec3::new(4, 63, -2), IVec3::new(0, 1, 0))),
        hotbar_slot: 3,
        held_rotation: 1,
        wishdir: Vec3::ZERO,
        jump: false,
        sprint: false,
    }));
    roundtrip(&ClientToServer::Action(PlayerAction::UseClick {
        mob: Some(812),
        target: Some(TargetRef::face(IVec3::new(4, 65, -2), IVec3::Y)),
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
    roundtrip(&ServerToClient::HelloAck {
        protocol: 50,
        challenge: [0x11; 32],
        requires_account: true,
        server_id: "0123456789abcdef0123456789abcdef".into(),
    });
    roundtrip(&ServerToClient::ModsDisabled {
        mods: vec!["farming".into(), "combat".into()],
    });
    for reason in [
        JoinRejectReason::BadProof,
        JoinRejectReason::InvalidName("Player name cannot be empty".into()),
        JoinRejectReason::AlreadyConnected,
        JoinRejectReason::ServerFull,
        JoinRejectReason::AccountRequired,
        JoinRejectReason::AccountRejected("That ticket has expired".into()),
    ] {
        roundtrip(&ServerToClient::JoinReject { reason });
    }
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
        fluid: None,
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
        sections: vec![
            TickSection::Creative(vec![crate::schematic::CreativeReply::Message(
                "Schematic placed".into(),
            )]),
            TickSection::Schematics(vec![crate::schematic::share::SchematicNotice::Ghost {
                key: "fixture:ghost".into(),
                placement: Some(crate::schematic::share::GhostPlacement {
                    digest: [7; 32],
                    origin: [-4, 60, 9],
                    turns: 3,
                    yields_to_positioning: false,
                }),
            }]),
            TickSection::BlockDeltas(vec![
                BlockDelta {
                    pos: IVec3::new(-8, 70, 3),
                    block_id: 9,
                    fluid: Some(0x87),
                    state: None,
                    cell_kv: vec![("furniture:dye".into(), vec![200, 30, 40])],
                },
                BlockDelta {
                    pos: IVec3::new(4, 65, 4),
                    block_id: 12,
                    fluid: None,
                    state: Some(petramond_world::block::ShapeState::with_ids(
                        &[1, 12, 0],
                        0b110,
                    )),
                    cell_kv: vec![],
                },
                BlockDelta {
                    pos: IVec3::new(5, 65, 4),
                    block_id: 30,
                    fluid: None,
                    state: Some(petramond_world::block::ShapeState::new(&[1, 0, 0, 3])),
                    cell_kv: vec![],
                },
            ]),
            TickSection::BlockDraws(vec![crate::net::protocol::BlockDrawDelta {
                pos: IVec3::new(1, 2, 3),
                prims: vec![mod_api::DrawPrim::Cuboid {
                    min: [0.0, 0.0, 0.0],
                    max: [1.0, 0.5, 1.0],
                    tile: "stone".into(),
                    tint: [200, 120, 60],
                    emissive: true,
                }]
                .into(),
            }]),
            TickSection::CellKvDeltas(vec![
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
            ]),
            TickSection::Mobs(
                vec![MobStateRow {
                    id: 4211,
                    kind_id: 1,
                    pos: WorldPos::new(4.5, 71.0, -2.25),
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
                    conditions: vec![(0, 1)],
                    anims: Vec::new(),
                    ragdoll: Some(vec![([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0])]),
                    dig: None,
                    held: [None; 2],
                    draw: Default::default(),
                }]
                .into(),
            ),
            TickSection::Items(EntityLane {
                despawned: vec![3, 5],
                spawned: vec![ItemStateRow {
                    id: 7,
                    item_id: 3,
                    count: 12,
                    data: None,
                    pos: WorldPos::new(0.5, 65.0, 0.5),
                    spin: 1.25,
                    flight: None,
                }]
                .into(),
                updated: RowSet::default(),
            }),
            TickSection::SleepTally(SleepTally {
                sleeping: 1,
                connected: 3,
            }),
            TickSection::Players(
                vec![PlayerStateRow {
                    conditions: Vec::new(),
                    id: PlayerId(1),
                    transform: Transform {
                        pos: WorldPos::new(4.5, 71.0, -2.25),
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
                    animator: crate::player::AnimatorClaims {
                        params: vec![crate::player::AnimatorParam {
                            rig: crate::player::RigId(0),
                            param: 4,
                            value: crate::player::AnimatorValue::Number(1.0),
                        }],
                        plays: vec![crate::player::AnimatorPlay {
                            rig: crate::player::RigId(0),
                            slot: 2,
                            clip: 9,
                            clock: crate::player::AnimatorClock::Scrub(0.25),
                            mirror: true,
                            priority: 1,
                        }],
                    },
                    hurt_recent: true,
                    snap: true,
                    mount: None,
                }]
                .into(),
            ),
            TickSection::PlayerActions(
                vec![
                    (PlayerId(1), PlayerActionKind::Died),
                    (PlayerId(0), PlayerActionKind::Respawned),
                    (
                        PlayerId(1),
                        PlayerActionKind::Animator {
                            rig: crate::player::RigId(0),
                            event: 5,
                        },
                    ),
                ]
                .into(),
            ),
            TickSection::SelfState(SelfState {
                conditions: Vec::new(),
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
                fly_scale: 1.0,
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
                animator: crate::player::AnimatorClaims {
                    params: Vec::new(),
                    plays: vec![crate::player::AnimatorPlay {
                        rig: crate::player::RigId(1),
                        slot: 0,
                        clip: 3,
                        clock: crate::player::AnimatorClock::Run {
                            rate: 1.0,
                            looping: false,
                        },
                        mirror: false,
                        priority: 0,
                    }],
                },
                sleeping: None,
                sleep_bed: None,
                transform: Some(SelfTransform {
                    transform: Transform {
                        pos: WorldPos::new(1.5, 80.0, -3.25),
                        vel: Vec3::ZERO,
                        yaw: 1.25,
                        pitch: -0.5,
                    },
                    on_ground: true,
                }),
            }),
            TickSection::OpenChests(vec![IVec3::new(1, 65, 1)]),
            TickSection::Env(vec![
                ("petramond:time".into(), [0.5, 1.0, 3.0, 0.0]),
                ("petramond:light".into(), [1.0, 1.0, 1.0, 1.0]),
            ]),
            TickSection::Events(vec![
                WorldEventMsg::BlockBroken {
                    pos: IVec3::new(4, 65, 4),
                    block_id: 12,
                    normal: Some(IVec3::Y),
                    tint: None,
                },
                WorldEventMsg::ItemPickedUp {
                    pos: WorldPos::new(1.0, 65.0, 2.0),
                    by: PlayerId(1),
                },
                WorldEventMsg::SpatialSound(SpatialSoundMsg::PlayOnMob {
                    handle: 3,
                    sound_id: 2,
                    mob_id: 4211,
                    volume: 0.5,
                    pitch: 1.1,
                    last_pos: WorldPos::new(0.0, 70.0, 0.0),
                }),
            ]),
            TickSection::SelfEvents(SelfEvents {
                picked_up_item: true,
                open_screen: Some(OpenScreen::Gui {
                    kind_key: "kitchen:oven".into(),
                    anchor: Some(crate::menu::MenuAnchor::Mob(7)),
                }),
                animator_events: vec![(crate::player::RigId(1), 2)],
                ..Default::default()
            }),
            TickSection::ActionOutcomes(vec![ActionOutcome {
                id: 1,
                accepted: true,
                reason: None,
            }]),
            TickSection::MenuSync(MenuSyncMsg {
                target: MenuTargetWire::Container {
                    kind_key: "kitchen:oven".into(),
                    anchor: Some(IVec3::new(4, 65, 4).into()),
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
        ],
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
            fluid: None,
            skylight: None,
            blocklight: None,
            states: Default::default(),
        };
        let bytes = postcard::to_allocvec(&payload).expect("encode");
        let back: SectionPayload = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(&back.blocks.0[..], &cells[..], "{} cells", cells.len());
    }
}

fn item(id: u64, x: f64) -> ItemStateRow {
    ItemStateRow {
        id,
        item_id: 1,
        count: 1,
        data: None,
        pos: WorldPos::new(x, 64.0, 0.0),
        spin: 0.0,
        flight: None,
    }
}

/// A selection over a shared table encodes ONLY its picks, in pick order, and
/// decodes to an owned set equal to it.
#[test]
fn a_row_selection_encodes_only_its_picks() {
    let table: Arc<[ItemStateRow]> = (0..6).map(|i| item(i, i as f64)).collect::<Vec<_>>().into();
    let picked = RowSet::select(Arc::clone(&table), vec![1, 4]);
    let owned: RowSet<ItemStateRow> = vec![item(1, 1.0), item(4, 4.0)].into();
    assert_eq!(
        postcard::to_allocvec(&picked).unwrap(),
        postcard::to_allocvec(&owned).unwrap(),
        "the wire never sees the table"
    );
    let back: RowSet<ItemStateRow> =
        postcard::from_bytes(&postcard::to_allocvec(&picked).unwrap()).unwrap();
    assert_eq!(back, picked);
    assert_eq!(back.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 4]);
}

/// Folding consecutive lanes equals applying them in order: a despawn wipes
/// the older row, a re-entry stays a spawn, the newest row per id wins.
#[test]
fn absorbing_a_newer_lane_composes_spawns_updates_and_despawns() {
    let mut lane: ItemLane = EntityLane {
        despawned: vec![9],
        spawned: vec![item(1, 0.0)].into(),
        updated: vec![item(2, 0.0), item(3, 0.0)].into(),
    };
    lane.absorb(EntityLane {
        despawned: vec![3, 1],
        spawned: vec![item(3, 5.0)].into(),
        updated: vec![item(2, 1.0)].into(),
    });
    assert_eq!(lane.despawned, [1, 3, 9]);
    let ids =
        |rows: &RowSet<ItemStateRow>| rows.iter().map(|r| (r.id, r.pos.x)).collect::<Vec<_>>();
    assert_eq!(ids(&lane.spawned), [(3, 5.0)], "3 left and came back");
    assert_eq!(
        ids(&lane.updated),
        [(2, 1.0)],
        "1 is gone; 2 took its newest row"
    );

    lane.absorb(EntityLane {
        despawned: Vec::new(),
        spawned: RowSet::default(),
        updated: vec![item(3, 6.0)].into(),
    });
    assert_eq!(
        ids(&lane.spawned),
        [(3, 6.0)],
        "an update after a spawn in the span is still a spawn"
    );
}
