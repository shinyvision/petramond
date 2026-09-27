use mod_api::calls;
use mod_api::{HostCall, HostRet, RuntimeSide};
use petramond_util::test_dirs::TestScratchDir;

use crate::modding::host::{handle_host_call, ModStoreData};

fn client_data(dir: &std::path::Path) -> ModStoreData {
    ModStoreData::new_for_side(
        "weathertest",
        7,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets::under(dir.to_owned())),
    )
}

/// The weather-era client calls: unknown keys are FORGIVING `false`
/// (a disabled pack is not a protocol break), malformed values are hard
/// errors, and the env-param read is capped at the GPU slot budget.
#[test]
fn weather_era_client_calls_validate_and_forgive() {
    let scratch = TestScratchDir::new("client-calls-weather-era");
    let mut data = client_data(&scratch);
    // Unknown bundle key / unknown sound key: forgiving false.
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientAmbientSet {
                key: "nope:rain".into(),
                intensity: 1.0,
                wind: [0.0, 0.0],
            }),
        ),
        HostRet::Bool(false)
    );
    // A real bundle that is NOT ambient (a shipped burst): also forgiving false.
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientAmbientSet {
                key: petramond_world::particle_emitters::defs()
                    .iter()
                    .find(|b| b.burst.is_some())
                    .expect("a burst bundle ships")
                    .key
                    .into(),
                intensity: 1.0,
                wind: [0.0, 0.0],
            }),
        ),
        HostRet::Bool(false)
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientLoopSet {
                key: "nope:loop".into(),
                gain: 1.0,
            }),
        ),
        HostRet::Bool(false)
    );
    // Non-finite / out-of-envelope values are hard errors.
    for bad in [
        HostCall::from(calls::ClientAmbientSet {
            key: "m:x".into(),
            intensity: f32::NAN,
            wind: [0.0, 0.0],
        }),
        HostCall::from(calls::ClientAmbientSet {
            key: "m:x".into(),
            intensity: 1.0,
            wind: [65.0, 0.0],
        }),
        HostCall::from(calls::ClientLoopSet {
            key: "m:x".into(),
            gain: f32::INFINITY,
        }),
        HostCall::from(calls::ClientMoodSet {
            darken: f32::NAN,
            desaturate: 0.0,
        }),
    ] {
        let ret = handle_host_call(&mut data, bad.clone());
        assert!(
            matches!(ret, HostRet::Err(_)),
            "malformed values must be a hard error: {bad:?} -> {ret:?}"
        );
    }
    // The mood clamps into its subtle envelope and always succeeds.
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientMoodSet {
                darken: 9.0,
                desaturate: -3.0,
            }),
        ),
        HostRet::Bool(true)
    );
    assert_eq!(data.client.as_ref().unwrap().mood, [0.5, 0.0]);
    // Env-param reads cap at the 16-slot GPU budget.
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientEnvParams {
                keys: (0..17).map(|i| format!("m:k{i}")).collect(),
            }),
        ),
        HostRet::Err(_)
    ));
}

/// The weather-era SERVER calls are rejected on a client instance by the
/// capability gate, like every sim-facing call.
#[test]
fn weather_era_server_calls_stay_server_side() {
    let scratch = TestScratchDir::new("client-calls-server-side");
    let mut data = client_data(&scratch);
    for call in [
        HostCall::from(calls::BiomeAt { pos: [0, 0] }),
        HostCall::from(calls::SurfaceYAt { pos: [0, 0] }),
        HostCall::from(calls::Players),
    ] {
        assert!(
            matches!(handle_host_call(&mut data, call), HostRet::Err(_)),
            "sim queries must be rejected on client instances"
        );
    }
}

/// `underground_biome_at` is a pure `(world seed, position)` partition, so a
/// presentation mod may ask which cave biome the camera is in and must get
/// the SERVER's answer — that is what lets a client mod drive an ambient
/// volume over an underground biome. Its sibling `terrain_solid_at` runs the
/// carve per position and stays server-side.
#[test]
fn the_underground_biome_partition_answers_on_a_client_instance() {
    let scratch = TestScratchDir::new("client-calls-underground-biome");
    let mut client = client_data(&scratch);
    let mut server = ModStoreData::new("weathertest", 7);
    let positions = vec![[0, -40, 0], [400, -30, -400], [-90, -20, 610]];
    let ask = |data: &mut ModStoreData| match handle_host_call(
        data,
        HostCall::from(calls::UndergroundBiomeAt {
            positions: positions.clone(),
        }),
    ) {
        HostRet::UndergroundBiomes(v) => v,
        other => panic!("expected the biome ids, got {other:?}"),
    };
    assert_eq!(
        ask(&mut client),
        ask(&mut server),
        "the client must see the same partition the carver did"
    );
    assert!(
        matches!(
            handle_host_call(
                &mut client,
                HostCall::from(calls::TerrainSolidAt {
                    positions: positions.clone()
                })
            ),
            HostRet::Err(_)
        ),
        "the carve query stays server-side"
    );
}

#[test]
fn client_instances_are_capability_isolated_and_namespace_their_state() {
    let scratch = TestScratchDir::new("unused-client-mod-test");
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets::under(
            scratch.to_path_buf(),
        )),
    );
    assert_eq!(
        handle_host_call(&mut data, HostCall::from(calls::RuntimeSide)),
        HostRet::RuntimeSide(RuntimeSide::Client)
    );
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::RegisterTickSystem {
                stage: mod_api::Stage::Mobs,
                attach: mod_api::AttachSide::After,
                priority: 0,
                system_id: 1,
            })
        ),
        HostRet::Err(_)
    ));
    assert!(data.pending.is_empty());
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientUiStateSet {
                key: "other:value".into(),
                value: mod_api::GuiValue::I32(1),
            })
        ),
        HostRet::Err(_)
    ));
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientUiStateSet {
                key: "map:value".into(),
                value: mod_api::GuiValue::I32(2),
            })
        ),
        HostRet::Unit
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientUiStateGet {
                key: "map:value".into(),
            })
        ),
        HostRet::GuiValue(Some(mod_api::GuiValue::I32(2)))
    );

    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientImageSet {
                key: "map:tile".into(),
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 255],
            }),
        ),
        HostRet::Unit
    );
    let image_revision = data
        .client
        .as_ref()
        .unwrap()
        .images
        .get("map:tile")
        .unwrap()
        .revision;
    let elements = vec![mod_api::ClientCanvasElement::Image {
        image_key: "map:tile".into(),
        rect: [0.0, 0.0, 160.0, 160.0],
    }];
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientCanvasSceneSet {
                canvas_key: "map:canvas".into(),
                elements: elements.clone(),
            }),
        ),
        HostRet::Unit
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientCanvasViewSet {
                canvas_key: "map:canvas".into(),
                offset: [12.0, -7.0],
            }),
        ),
        HostRet::Unit
    );
    let client = data.client.as_ref().unwrap();
    let scene = client.canvas_scenes.get("map:canvas").unwrap();
    assert_eq!(*scene.elements, elements);
    assert_eq!(scene.offset, [12.0, -7.0]);
    assert_eq!(client.images["map:tile"].revision, image_revision);
}

#[test]
fn client_image_blit_mutates_in_place_and_validates_bounds() {
    let scratch = TestScratchDir::new("unused-client-blit-test");
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets::under(
            scratch.to_path_buf(),
        )),
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientImageSet {
                key: "map:tile".into(),
                width: 2,
                height: 2,
                rgba: vec![0; 16],
            }),
        ),
        HostRet::Unit
    );
    let revision = data.client.as_ref().unwrap().images["map:tile"].revision;
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientImageBlit {
                key: "map:tile".into(),
                origin: [1, 1],
                size: [1, 1],
                rgba: vec![9, 8, 7, 255],
            }),
        ),
        HostRet::Unit
    );
    let image = &data.client.as_ref().unwrap().images["map:tile"];
    assert_eq!(&image.rgba[12..16], &[9, 8, 7, 255], "blit lands at (1,1)");
    assert_eq!(&image.rgba[0..4], &[0, 0, 0, 0], "pixels outside stay");
    assert_ne!(image.revision, revision, "a blit must move the revision");
    assert_eq!(
        image.recent_blits,
        vec![(image.revision, [1, 1, 1, 1])],
        "the blit records its rect for partial texture uploads"
    );

    // The partial-update chain: bounded window, oldest first, broken by
    // whole-image mutations (text draws, re-publish).
    for _ in 0..super::super::state::IMAGE_BLIT_WINDOW + 2 {
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientImageBlit {
                key: "map:tile".into(),
                origin: [0, 0],
                size: [1, 1],
                rgba: vec![1, 1, 1, 255],
            }),
        );
    }
    let image = &data.client.as_ref().unwrap().images["map:tile"];
    assert_eq!(
        image.recent_blits.len(),
        super::super::state::IMAGE_BLIT_WINDOW
    );
    assert!(
        image.recent_blits.windows(2).all(|w| w[1].0 == w[0].0 + 1),
        "window entries stay consecutive"
    );
    assert_eq!(image.recent_blits.last().unwrap().0, image.revision);
    handle_host_call(
        &mut data,
        HostCall::from(calls::ClientImageDrawTexts {
            key: "map:tile".into(),
            runs: vec![mod_api::ClientTextRun {
                text: "x".into(),
                position: [0, 0],
                scale: 1,
                color: [255, 255, 255, 255],
            }],
        }),
    );
    let image = &data.client.as_ref().unwrap().images["map:tile"];
    assert!(
        image.recent_blits.is_empty(),
        "text draws break the partial chain (no rect is tracked for them)"
    );

    for bad in [
        // out of bounds
        HostCall::from(calls::ClientImageBlit {
            key: "map:tile".into(),
            origin: [2, 0],
            size: [1, 1],
            rgba: vec![0; 4],
        }),
        // byte count mismatch
        HostCall::from(calls::ClientImageBlit {
            key: "map:tile".into(),
            origin: [0, 0],
            size: [1, 1],
            rgba: vec![0; 3],
        }),
        // never published
        HostCall::from(calls::ClientImageBlit {
            key: "map:none".into(),
            origin: [0, 0],
            size: [1, 1],
            rgba: vec![0; 4],
        }),
        // foreign namespace
        HostCall::from(calls::ClientImageBlit {
            key: "other:tile".into(),
            origin: [0, 0],
            size: [1, 1],
            rgba: vec![0; 4],
        }),
    ] {
        assert!(matches!(handle_host_call(&mut data, bad), HostRet::Err(_)));
    }
}

#[test]
fn client_surface_columns_gate_on_revision_and_pack_cells() {
    let scratch = TestScratchDir::new("unused-client-surface-test");
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets::under(
            scratch.to_path_buf(),
        )),
    );
    let mut world = crate::world::ReplicaWorld::new(0, 0);
    let sp = petramond_world::chunk::SectionPos::new(0, 4, 0);
    world.insert_section_for_test(sp, petramond_world::section::Section::new(0, 4, 0));
    assert!(world.set_block_world(3, 64, 5, petramond_world::block::Block::Stone));

    let query = |revision| {
        HostCall::from(calls::ClientSurfaceColumns {
            queries: vec![
                mod_api::ClientSurfaceQuery {
                    coord: [0, 0],
                    revision,
                },
                mod_api::ClientSurfaceQuery {
                    coord: [9, 9],
                    revision: 0,
                },
            ],
        })
    };
    let HostRet::ClientSurfaceColumns(replies) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query(0)))
    else {
        panic!("surface columns reply expected");
    };
    assert!(replies[1].is_none(), "an unloaded column replies None");
    let column = replies[0].as_ref().expect("loaded column");
    let cells = column.cells.as_ref().expect("first sight sends cells");
    assert_eq!(cells.len(), mod_api::CLIENT_SURFACE_COLUMN_BYTES);
    let cell = |lx: usize, lz: usize| {
        let at = (lz * 16 + lx) * mod_api::CLIENT_SURFACE_CELL_BYTES;
        i16::from_le_bytes([cells[at], cells[at + 1]])
    };
    assert_eq!(cell(3, 5), 64, "the placed surface cell is known");
    assert_eq!(
        cell(0, 0),
        mod_api::CLIENT_SURFACE_UNKNOWN_HEIGHT,
        "cells with no surface stay unknown"
    );

    // Echoing the served revision skips the cell payload…
    let revision = column.revision;
    let HostRet::ClientSurfaceColumns(replies) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query(revision)))
    else {
        panic!("surface columns reply expected");
    };
    let unchanged = replies[0].as_ref().expect("loaded column");
    assert_eq!(unchanged.revision, revision);
    assert!(unchanged.cells.is_none(), "unchanged column sends no cells");

    // …until an edit moves the column revision.
    assert!(world.set_block_world(3, 64, 5, petramond_world::block::Block::Dirt));
    let HostRet::ClientSurfaceColumns(replies) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query(revision)))
    else {
        panic!("surface columns reply expected");
    };
    let changed = replies[0].as_ref().expect("loaded column");
    assert_ne!(changed.revision, revision);
    assert!(changed.cells.is_some(), "a moved revision resends cells");
}

#[test]
fn client_blocks_at_reads_the_replica_and_gates_on_stream_finality() {
    let scratch = TestScratchDir::new("unused-client-blocks-test");
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets::under(
            scratch.to_path_buf(),
        )),
    );
    let mut world = crate::world::ReplicaWorld::new(0, 0);
    let sp = petramond_world::chunk::SectionPos::new(0, 4, 0);
    world.insert_section_for_test(sp, petramond_world::section::Section::new(0, 4, 0));
    assert!(world.set_block_world(3, 64, 5, petramond_world::block::Block::Stone));

    let query = || {
        HostCall::from(calls::ClientBlocksAt {
            positions: vec![[3, 64, 5], [3, 65, 5], [150, 64, 5]],
        })
    };
    let HostRet::Blocks(blocks) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query()))
    else {
        panic!("blocks reply expected");
    };
    assert_eq!(
        blocks[0],
        Some(mod_api::BlockId(petramond_world::block::Block::Stone.id()))
    );
    assert_eq!(
        blocks[1],
        Some(mod_api::BlockId(petramond_world::block::Block::Air.id()))
    );
    assert_eq!(blocks[2], None, "an unloaded section reads None");

    // A section whose streamed content is not final reads None — the same
    // "state frozen, retry later" contract as the server-side mod reads.
    world.mark_overlay_in_flight_for_test(sp);
    let HostRet::Blocks(blocks) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query()))
    else {
        panic!("blocks reply expected");
    };
    assert_eq!(
        blocks[0], None,
        "an in-flight overlay leaked a replica read"
    );

    // Registry-only queries are legal on client instances (a client mod
    // interpreting block ids has to resolve the names and tag sets it
    // compares to).
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ResolveBlock {
                name: "petramond:stone".into()
            })
        ),
        HostRet::Block(Some(mod_api::BlockId(
            petramond_world::block::Block::Stone.id()
        )))
    );
    let HostRet::BlockList(leaves) = handle_host_call(
        &mut data,
        HostCall::from(calls::BlocksByTag {
            tag: "petramond:leaves".into(),
        }),
    ) else {
        panic!("block list expected");
    };
    assert!(!leaves.is_empty());

    // The batch bound is enforced.
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientBlocksAt {
                positions: vec![[0, 0, 0]; 513],
            })
        ),
        HostRet::Err(_)
    ));
}

/// A client instance may pose only the LOCAL player, and a hand becomes
/// client-authoritative on its first NON-empty pose.
///
/// The latch rule is the whole reason the prediction composes: a mod that
/// publishes "no pose" every frame (the shield you are not carrying) must NOT
/// claim the hand, or it would blank a pose another pack set server-side; and
/// once it HAS posed a hand, releasing must present locally rather than wait a
/// round trip for the authority to agree.
#[test]
fn a_client_poses_only_the_local_player_and_latches_a_hand_on_its_first_pose() {
    use mod_api::{HeldPose, HeldPoseData, PlayerId, PlayerSnapshot};
    use petramond_world::inventory::Hand;

    let scratch = TestScratchDir::new("client-calls-held-pose");
    let mut data = client_data(&scratch);
    let guard = HeldPose {
        first_person: HeldPoseData {
            rotation: [0.0; 3],
            translation: [0.0, 7.0, -2.0],
        },
        third_person: HeldPoseData::IDENTITY,
    };
    let pose = |data: &mut ModStoreData, player: u8, main, off| {
        handle_host_call(
            data,
            HostCall::from(calls::SetPlayerHeldPose {
                player: PlayerId(player),
                main,
                off,
            }),
        )
    };
    let local = PlayerSnapshot {
        id: Some(PlayerId(3)),
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health: 20,
        on_ground: true,
        spectator: false,
        sneak: false,
        use_held: false,
        holds_use: false,
        held: None,
        off_held: None,
        held_count: 0,
        pose_anchor: None,
        swing: Default::default(),
        half_width: 0.3,
        height: 1.8,
        eye_height: 1.62,
        entombed: false,
        conditions: Vec::new(),
    };

    crate::modding::client::scope::enter_actor(local, || {
        // Someone else's body is not this mirror's to pose.
        assert!(matches!(
            pose(&mut data, 4, Some(guard), None),
            HostRet::Err(_)
        ));

        // "Nothing in either hand" claims nothing: another pack's replicated
        // pose still owns both hands.
        assert_eq!(pose(&mut data, 3, None, None), HostRet::Bool(true));
        assert_eq!(
            data.client.as_ref().unwrap().poses_hands,
            [false, false],
            "an empty publish must not claim a hand"
        );

        // The first real pose claims that hand — and only that hand.
        assert_eq!(pose(&mut data, 3, Some(guard), None), HostRet::Bool(true));
        assert_eq!(data.client.as_ref().unwrap().poses_hands, [true, false]);

        // Releasing keeps the claim, so the drop presents on this frame.
        assert_eq!(pose(&mut data, 3, None, None), HostRet::Bool(true));
        assert_eq!(data.client.as_ref().unwrap().poses_hands, [true, false]);
        assert_eq!(
            data.client.as_ref().unwrap().body.held_pose(Hand::Main),
            None,
            "the claim outlives the pose; the pose itself is gone"
        );

        // A NaN pose is refused whole, exactly as on the server.
        let mut nan = guard;
        nan.third_person.translation[2] = f32::NAN;
        assert!(matches!(
            pose(&mut data, 3, Some(nan), None),
            HostRet::Err(_)
        ));
    });
}

/// Bone offsets latch PER BONE, and their names resolve to rig ids at the call.
///
/// Per bone because they COMPOSE: a pack predicting a shoulder must leave
/// another pack's replicated head tilt exactly where it was, which a body-wide
/// latch cannot express — it would blank every bone the predicting mod never
/// touched. A name the rig does not carry is dropped rather than refused, so
/// one stale bone in a list does not cost the caller the rest of its stance.
#[test]
fn bone_poses_resolve_to_rig_ids_and_latch_per_bone() {
    use mod_api::{BonePoseData, BonePoseMode, PlayerId, PlayerSnapshot};

    let scratch = TestScratchDir::new("client-calls-bone-pose");
    let mut data = client_data(&scratch);
    let bend = |bone: &str| BonePoseData {
        bone: bone.into(),
        rotation: [-22.0, 0.0, 0.0],
        translation: [0.0; 3],
        mode: BonePoseMode::Replace,
    };
    let set = |data: &mut ModStoreData, bones: Vec<BonePoseData>| {
        handle_host_call(
            data,
            HostCall::from(calls::SetPlayerBonePose {
                player: PlayerId(3),
                bones,
            }),
        )
    };
    let local = PlayerSnapshot {
        id: Some(PlayerId(3)),
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health: 20,
        on_ground: true,
        spectator: false,
        sneak: false,
        use_held: false,
        holds_use: false,
        held: None,
        off_held: None,
        held_count: 0,
        pose_anchor: None,
        swing: Default::default(),
        half_width: 0.3,
        height: 1.8,
        eye_height: 1.62,
        entombed: false,
        conditions: Vec::new(),
    };
    let want = crate::player::model::bone_id(mod_api::bone::MAIN_SHOULDER)
        .expect("the rig carries the main arm");

    crate::modding::client::scope::enter_actor(local, || {
        // An empty publish claims nothing — another pack's replicated bend
        // still owns every bone.
        assert_eq!(set(&mut data, Vec::new()), HostRet::Bool(true));
        assert!(data.client.as_ref().unwrap().poses_bones.is_empty());

        // A real bend latches THAT bone and nothing else, stored by id.
        assert_eq!(
            set(&mut data, vec![bend(mod_api::bone::MAIN_SHOULDER)]),
            HostRet::Bool(true)
        );
        let client = data.client.as_ref().unwrap();
        assert_eq!(
            client.poses_bones.iter().copied().collect::<Vec<_>>(),
            [want]
        );
        assert_eq!(
            client.body.bone_poses().map(|b| b.bone).collect::<Vec<_>>(),
            [want]
        );

        // Releasing keeps the latch (the drop must present on THIS frame),
        // and a bone the rig does not have is dropped, not an error.
        assert_eq!(
            set(&mut data, vec![bend("no_such_bone")]),
            HostRet::Bool(true)
        );
        let client = data.client.as_ref().unwrap();
        assert_eq!(client.body.bone_poses().count(), 0, "unknown names drop");
        assert_eq!(
            client.poses_bones.iter().copied().collect::<Vec<_>>(),
            [want],
            "the latch outlives the offset"
        );

        // A NaN is refused whole, exactly as on the server.
        let mut nan = bend(mod_api::bone::MAIN_SHOULDER);
        nan.rotation[1] = f32::NAN;
        assert!(matches!(set(&mut data, vec![nan]), HostRet::Err(_)));
    });
}

/// `PlayerInventory` on a client instance answers only the LOCAL player,
/// only while a dispatch has published the replicated inventory — and in
/// the one carried layout the server's read shares (grid, then off hand).
#[test]
fn client_player_inventory_is_local_only_and_needs_a_published_inventory() {
    use crate::modding::client::scope;
    use petramond_world::inventory::{Inventory, TOTAL_SLOTS};
    use petramond_world::item::{ItemStack, ItemType};

    let scratch = TestScratchDir::new("client-calls-inventory-gate");
    let mut data = client_data(&scratch);
    let me = mod_api::PlayerId(7);
    let query = |player| HostCall::from(calls::PlayerInventory { player });
    let actor = |id| mod_api::PlayerSnapshot {
        id: Some(id),
        ..blank_snapshot()
    };

    assert!(
        matches!(handle_host_call(&mut data, query(me)), HostRet::Err(_)),
        "no dispatch published an actor"
    );
    let mut inventory = Inventory::new();
    *inventory.slot_mut(0).unwrap() = Some(ItemStack::new(ItemType::Stone, 3));
    *inventory.off_hand_mut() = Some(ItemStack::new(ItemType::Dirt, 1));

    scope::enter_actor(actor(me), || {
        assert!(
            matches!(handle_host_call(&mut data, query(me)), HostRet::Err(_)),
            "an actor without a published inventory is an error, not an empty read"
        );
        scope::enter_inventory(&inventory, || {
            assert!(
                matches!(
                    handle_host_call(&mut data, query(mod_api::PlayerId(8))),
                    HostRet::Err(_)
                ),
                "somebody else's inventory is not a client read"
            );
            let HostRet::ContainerSlots(Some(slots)) = handle_host_call(&mut data, query(me))
            else {
                panic!("the local inventory reads as carried slots");
            };
            assert_eq!(slots.len(), TOTAL_SLOTS + 1);
            assert_eq!(slots[0].as_ref().map(|s| s.count), Some(3));
            assert_eq!(
                slots[TOTAL_SLOTS].as_ref().map(|s| s.item.as_str()),
                Some("petramond:dirt"),
                "the off hand is the LAST carried entry"
            );
        });
    });
}

/// A refused animator write latches NOTHING: the key stays replicated, so
/// one NaN from a client mod cannot hide the server's claim on that param
/// for the rest of the session. A good write then latches its keys.
#[test]
fn a_refused_animator_write_latches_no_key() {
    use mod_api::{AnimatorParam, AnimatorValue, PlayerId};

    let scratch = TestScratchDir::new("client-calls-animator-latch");
    let mut data = client_data(&scratch);
    let rig = crate::player::rigs::id(mod_api::rig::PLAYER_BODY).expect("the body rig");
    let graph = crate::player::rigs::graph(rig).expect("the body rig has an animator");
    let param = graph
        .param_names()
        .next()
        .expect("the body graph declares a param")
        .to_string();
    let set = |data: &mut ModStoreData, value: f32| {
        handle_host_call(
            data,
            HostCall::from(calls::SetPlayerAnimatorParams {
                player: PlayerId(3),
                params: vec![AnimatorParam {
                    rig: mod_api::rig::PLAYER_BODY.into(),
                    param: param.clone(),
                    value: AnimatorValue::Number(value),
                }],
            }),
        )
    };
    let mut local = blank_snapshot();
    local.id = Some(PlayerId(3));
    crate::modding::client::scope::enter_actor(local, || {
        assert!(matches!(set(&mut data, f32::NAN), HostRet::Err(_)));
        assert!(data
            .client
            .as_ref()
            .unwrap()
            .owns_animator
            .params
            .is_empty());
        assert_eq!(set(&mut data, 1.0), HostRet::Bool(true));
        assert_eq!(data.client.as_ref().unwrap().owns_animator.params.len(), 1);
    });
}

/// A snapshot with nothing in it — the fixture an actor-gated call test
/// stamps an id onto.
fn blank_snapshot() -> mod_api::PlayerSnapshot {
    mod_api::PlayerSnapshot {
        id: None,
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health: 20,
        on_ground: true,
        spectator: false,
        sneak: false,
        use_held: false,
        holds_use: false,
        held: None,
        off_held: None,
        held_count: 0,
        pose_anchor: None,
        swing: Default::default(),
        half_width: 0.3,
        height: 1.8,
        eye_height: 1.62,
        entombed: false,
        conditions: Vec::new(),
    }
}

/// Every VIEW call and every capability call this surface declares, one
/// sample each, in declaration order.
fn view_and_session_calls() -> Vec<HostCall> {
    vec![
        HostCall::from(calls::ClientViewCameraSet {
            pos: [1.0, 2.0, 3.0],
            yaw: 0.5,
            pitch: -0.25,
            roll: 0.0,
            fov_y: Some(1.2),
            anchor: None,
        }),
        HostCall::from(calls::ClientViewCameraRelease),
        HostCall::from(calls::ClientViewChromeSet {
            hud: Some(false),
            hands: None,
            crosshair: Some(true),
        }),
        HostCall::from(calls::ClientViewPerspectiveSet {
            third_person: Some(true),
        }),
        HostCall::from(calls::ClientViewState),
        HostCall::from(calls::ClientEnvSet {
            params: vec![("weathertest:tint".into(), [1.0, 1.0, 1.0, 1.0])],
        }),
        HostCall::from(calls::ClientKeyLabels {
            ids: vec!["open".into()],
        }),
        HostCall::from(calls::ClientStorageWritePoll {
            scope: mod_api::ClientStorageScope::Pack,
            ticket: 1,
        }),
        HostCall::from(calls::ClientUiFocus {
            id: "name".into(),
            item: None,
        }),
        HostCall::from(calls::ClientPauseOpen),
        HostCall::from(calls::ClientViewFrameSet {
            size: Some([640, 360]),
        }),
        HostCall::from(calls::ClientViewSubjectSet {
            player: Some(mod_api::PlayerId(1)),
        }),
        HostCall::from(calls::ClientEntities {
            ids: Vec::new(),
            near: None,
        }),
    ]
    .into_iter()
    .chain(capability_calls())
    .collect()
}

/// One sample of every capability call: files, state, events, presentation,
/// frames and the clock, taps, media and facts.
fn capability_calls() -> Vec<HostCall> {
    use mod_api::ClientStorageScope::Pack;
    let ranges = || {
        vec![mod_api::ClientFileRanges {
            scope: Pack,
            path: "r/w.pmc".into(),
            ranges: vec![[0, 40]],
        }]
    };
    let pose = mod_api::ClientPose {
        pos: [1.0, 70.0, 2.0],
        yaw: 0.5,
        pitch: 0.0,
    };
    vec![
        HostCall::from(calls::ClientFileAppend {
            scope: Pack,
            path: "a".into(),
            bytes: vec![1],
        }),
        HostCall::from(calls::ClientFileWrite {
            scope: Pack,
            path: "a".into(),
            offset: 0,
            bytes: vec![1],
            truncate: true,
        }),
        HostCall::from(calls::ClientFileSync {
            scope: Pack,
            path: "a".into(),
        }),
        HostCall::from(calls::ClientFileRename {
            scope: Pack,
            from: "a".into(),
            to: "b".into(),
        }),
        HostCall::from(calls::ClientFileDelete {
            scope: Pack,
            path: "b".into(),
        }),
        HostCall::from(calls::ClientFileRead {
            scope: Pack,
            path: "a".into(),
            offset: 0,
            len: 16,
        }),
        HostCall::from(calls::ClientFileList {
            scope: Pack,
            dir: "".into(),
            after: None,
            max_bytes: 4096,
        }),
        HostCall::from(calls::ClientFilePoll { ticket: 1 }),
        HostCall::from(calls::ClientFileStat {
            scope: Pack,
            path: "a".into(),
        }),
        HostCall::from(calls::ClientFileReveal {
            scope: Pack,
            path: "a".into(),
        }),
        HostCall::from(calls::ClientWorldStateWrite {
            scope: Pack,
            path: "r/w.pmc".into(),
            select: mod_api::ClientStateSelect::All,
            kinds: None,
            envelopes: Some("r/w.env".into()),
        }),
        HostCall::from(calls::ClientWorldEventsBegin {
            scope: Pack,
            path: "r/e.pmc".into(),
            envelopes: None,
        }),
        HostCall::from(calls::ClientWorldEventsEnd { events: 1 }),
        HostCall::from(calls::ClientWorldEventsPoll { events: 1 }),
        HostCall::from(calls::ClientPresentationOpen {
            tables: mod_api::ClientFileRange {
                scope: Pack,
                path: "r/w.pmc".into(),
                offset: 40,
                len: 80,
            },
            seed: 7,
            mods: Vec::new(),
            viewer: Some(pose),
        }),
        HostCall::from(calls::ClientPresentationApply {
            state: ranges(),
            events: ranges(),
            at: 1.5,
        }),
        HostCall::from(calls::ClientPresentationCancel { apply: 1 }),
        HostCall::from(calls::ClientPresentationQueue { events: ranges() }),
        HostCall::from(calls::ClientPresentationTime { at: 2.0 }),
        HostCall::from(calls::ClientPresentationViewer { pose, flying: true }),
        HostCall::from(calls::ClientPresentationState),
        HostCall::from(calls::ClientPresentationClose),
        HostCall::from(calls::ClientFrameCapture {
            source: mod_api::ClientCaptureSource::World,
            size: Some([160, 90]),
            when: mod_api::ClientCaptureWhen::Settled,
            advance: false,
            into: mod_api::ClientCaptureInto::File {
                scope: Pack,
                path: "thumb.raw".into(),
            },
        }),
        HostCall::from(calls::ClientFrameCapturePoll { capture: 1 }),
        HostCall::from(calls::ClientFrameCancel { capture: 1 }),
        HostCall::from(calls::ClientClockSet { step: None }),
        HostCall::from(calls::ClientClockAdvance),
        HostCall::from(calls::ClientAudioTap {
            sample_rate: 48_000,
            channels: 2,
            into: mod_api::ClientAudioInto::File {
                scope: Pack,
                path: "world.pcm".into(),
            },
        }),
        HostCall::from(calls::ClientAudioTapState { tap: 1 }),
        HostCall::from(calls::ClientAudioTapEnd { tap: 1 }),
        HostCall::from(calls::ClientMediaEncoders { refresh: false }),
        HostCall::from(calls::ClientMediaOpen {
            scope: Pack,
            path: "videos/a.mp4".into(),
            container: "mp4".into(),
            video: Some(mod_api::ClientMediaVideo {
                codec: "libx264".into(),
                width: 64,
                height: 36,
                fps: [30, 1],
                options: Vec::new(),
            }),
            audio: None,
            options: Vec::new(),
        }),
        HostCall::from(calls::ClientMediaPushFrame {
            media: 1,
            rgba: vec![0; 4],
        }),
        HostCall::from(calls::ClientMediaPushAudio {
            media: 1,
            pcm: vec![0; 4],
        }),
        HostCall::from(calls::ClientMediaClose { media: 1 }),
        HostCall::from(calls::ClientMediaAbort { media: 1 }),
        HostCall::from(calls::ClientMediaState { media: 1 }),
        HostCall::from(calls::ClientEngineFacts),
        HostCall::from(calls::ClientPacks),
        HostCall::from(calls::ClientWallClock),
    ]
}

/// A client-legal call that is PERMITTED but not ROUTED falls through to the
/// simulation handler and dies on "no simulation context is active" — a trap
/// that costs nothing to fall into when a variant is appended and nothing to
/// catch here. The mirror invariant: on a SIM instance every one of them is a
/// clean refusal, never a panic and never a partial write.
#[test]
fn the_view_and_capability_calls_are_routed_on_a_client_and_refused_on_a_sim() {
    let scratch = TestScratchDir::new("client-calls-view-session");
    let mut client = client_data(&scratch);
    for call in view_and_session_calls() {
        let name = format!("{call:?}");
        if let HostRet::Err(mod_api::HostError {
            detail: message, ..
        }) = handle_host_call(&mut client, call)
        {
            assert!(
                !message.contains("simulation"),
                "{name} reached the simulation handler: {message}"
            );
        }
    }

    let scratch = TestScratchDir::new("client-calls-view-session-sim");
    let mut sim = ModStoreData::new_for_side(
        "weathertest",
        7,
        RuntimeSide::Server,
        Some(crate::modding::client::ClientBuckets::under(
            scratch.to_path_buf(),
        )),
    );
    for call in view_and_session_calls() {
        let name = format!("{call:?}");
        assert!(
            matches!(handle_host_call(&mut sim, call), HostRet::Err(_)),
            "{name} answered a sim instance"
        );
    }
}

/// While a presentation is on screen, what a mod sees is the presented
/// world's: a `World` write made then — a KV write or a file — never reaches
/// the session's own bucket, while reads keep answering from it.
#[test]
fn client_storage_writes_during_a_presentation_never_reach_the_session() {
    use mod_api::ClientStorageScope::World;
    let dir = TestScratchDir::new("client-calls-presentation-storage");
    let store = |dir: &std::path::Path| {
        ModStoreData::new_for_side(
            "map",
            7,
            RuntimeSide::Client,
            Some(crate::modding::client::ClientBuckets::under(dir.to_owned())),
        )
    };
    let set = |data: &mut ModStoreData, value: u8| {
        handle_host_call(
            data,
            HostCall::from(calls::ClientStorageSetMany {
                scope: World,
                entries: vec![("map:tile".into(), Some(mod_api::ByteBuf::from(vec![value])))],
            }),
        )
    };
    let append = |data: &mut ModStoreData, value: u8| {
        handle_host_call(
            data,
            HostCall::from(calls::ClientFileAppend {
                scope: World,
                path: "tiles/0".into(),
                bytes: vec![value],
            }),
        )
    };
    let get = |data: &mut ModStoreData| {
        handle_host_call(
            data,
            HostCall::from(calls::ClientStorageGetMany {
                scope: World,
                keys: vec!["map:tile".into()],
            }),
        )
    };
    let holds =
        |value: u8| HostRet::ClientStorageValues(vec![Some(mod_api::ByteBuf::from(vec![value]))]);

    let mut data = store(&dir);
    assert!(matches!(set(&mut data, 1), HostRet::ClientStorageWrite(_)));
    assert!(matches!(append(&mut data, 1), HostRet::Ticket(_)));
    let presented = data.client.as_ref().unwrap().presented.clone();
    let live = std::mem::replace(
        &mut presented.lock().context,
        mod_api::ClientContext::Presentation {
            owner: "map".into(),
        },
    );
    assert!(
        matches!(
            set(&mut data, 2),
            HostRet::Err(mod_api::HostError {
                code: mod_api::ErrorCode::Refused,
                ..
            })
        ),
        "refused, and says so"
    );
    assert!(matches!(
        append(&mut data, 2),
        HostRet::Err(mod_api::HostError {
            code: mod_api::ErrorCode::Refused,
            ..
        })
    ));
    assert_eq!(
        get(&mut data),
        holds(1),
        "reads answer from the session's storage"
    );
    presented.lock().context = live;
    assert_eq!(get(&mut data), holds(1));
    drop(data);
    assert_eq!(
        get(&mut store(&dir)),
        holds(1),
        "the disk never saw the presentation's write"
    );
    // A read is ordered after the writes queued to its bytes.
    let (tx, rx) = std::sync::mpsc::channel();
    let tile = crate::modding::client::files::FileRef::in_bucket(
        &dir.join("world").join("files"),
        "tiles/0",
    );
    crate::modding::client::files::read(&tile, 0, 1, move |read| {
        let _ = tx.send(read);
    });
    assert_eq!(
        rx.recv().unwrap(),
        Ok(vec![1]),
        "an instance's files reach the disk once it shuts down"
    );
}

/// World marks are accepted WHOLE or refused WHOLE — a mod that sends an
/// invalid mark keeps the set it had, never a truncated one — and the call is
/// routed on a client and refused on a sim instance.
#[test]
fn a_world_mark_set_is_kept_whole_or_refused_whole() {
    use mod_api::{ClientSprite, ClientWorldMark};
    let line = |width: f32| ClientWorldMark::Line {
        from: [0.0, 64.0, 0.0],
        to: [f64::from(1u32 << 31), 1e12, -3.0],
        color: [255, 0, 0, 255],
        width,
        occluded: 0,
    };
    let point = |sprite: ClientSprite, label: &str| ClientWorldMark::Point {
        pos: [1.0, 2.0, 3.0],
        sprite: Some(sprite),
        size: 16.0,
        color: [255; 4],
        label: Some(label.into()),
        occluded: 255,
    };
    let scratch = TestScratchDir::new("client-calls-world-marks");
    let mut data = client_data(&scratch);
    let set = |data: &mut ModStoreData, marks: Vec<ClientWorldMark>| {
        handle_host_call(
            data,
            HostCall::from(calls::ClientWorldMarksSet {
                set: "path".into(),
                marks,
            }),
        )
    };
    let own = point(
        ClientSprite::Image {
            key: "weathertest:pin".into(),
        },
        "home",
    );
    assert_eq!(set(&mut data, vec![line(2.0), own.clone()]), HostRet::Unit);
    let kept = data.client.as_ref().unwrap().world_marks["path"].clone();
    assert_eq!(kept.len(), 2);
    // Points land inside the world border on every axis.
    let border = f64::from(petramond_world::border::WORLD_BORDER);
    let ClientWorldMark::Line { to, .. } = kept[0] else {
        panic!("the line came back as {:?}", kept[0]);
    };
    assert!(to.iter().all(|c| c.abs() <= border), "{to:?}");

    let refused = [
        vec![line(0.0)],
        vec![line(f32::NAN)],
        vec![ClientWorldMark::Line {
            from: [f64::NAN, 0.0, 0.0],
            to: [0.0; 3],
            color: [0; 4],
            width: 1.0,
            occluded: 0,
        }],
        vec![point(
            ClientSprite::Image {
                key: "othermod:pin".into(),
            },
            "",
        )],
        vec![point(
            ClientSprite::Theme {
                part: "icon.plus".into(),
            },
            "two\nlines",
        )],
    ];
    for marks in refused {
        let what = format!("{:?}", marks.first());
        assert!(
            matches!(set(&mut data, marks), HostRet::Err(_)),
            "accepted {what}"
        );
        assert_eq!(
            data.client.as_ref().unwrap().world_marks["path"],
            kept,
            "a refused set changed the kept one ({what})"
        );
    }
    // A second set is replaced on its own.
    let live = HostCall::from(calls::ClientWorldMarksSet {
        set: "live".into(),
        marks: vec![line(1.0)],
    });
    assert_eq!(handle_host_call(&mut data, live), HostRet::Unit);
    let bad_name = HostCall::from(calls::ClientWorldMarksSet {
        set: "Live Path".into(),
        marks: Vec::new(),
    });
    assert!(matches!(
        handle_host_call(&mut data, bad_name),
        HostRet::Err(_)
    ));
    assert_eq!(set(&mut data, Vec::new()), HostRet::Unit);
    let left: Vec<&String> = data.client.as_ref().unwrap().world_marks.keys().collect();
    assert_eq!(left, ["live"], "clearing one set leaves the other");

    let scratch = TestScratchDir::new("client-calls-world-marks-sim");
    let mut sim = ModStoreData::new_for_side(
        "weathertest",
        7,
        RuntimeSide::Server,
        Some(crate::modding::client::ClientBuckets::under(
            scratch.to_path_buf(),
        )),
    );
    assert!(matches!(set(&mut sim, vec![line(2.0)]), HostRet::Err(_)));
}

fn shell_data(dir: &std::path::Path) -> ModStoreData {
    ModStoreData::new_for_side(
        "map",
        0,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets {
            world: None,
            pack: dir.join("pack"),
        }),
    )
}

/// On the shell there is no world: every call that reads or writes one is
/// refused with an error that says so — none reaches a desk or a disk — while
/// the calls a launched tool is made of (its UI, its pack bucket, its files)
/// answer as they do anywhere.
#[test]
fn a_shell_instance_is_refused_every_world_call_cleanly() {
    use mod_api::{ClientContext, ClientStorageScope};
    let dir = TestScratchDir::new("client-calls-shell");
    let mut data = shell_data(&dir);

    let world_calls = [
        HostCall::from(calls::ClientBlocksAt {
            positions: vec![[0, 0, 0]],
        }),
        HostCall::from(calls::ClientSurfaceColumns { queries: vec![] }),
        HostCall::from(calls::ClientEnvParams { keys: vec![] }),
        HostCall::from(calls::ClientViewState),
        HostCall::from(calls::ClientViewCameraRelease),
        HostCall::from(calls::ClientWorldMarksSet {
            set: "path".into(),
            marks: vec![],
        }),
        HostCall::from(calls::ClientMoodSet {
            darken: 0.1,
            desaturate: 0.1,
        }),
        HostCall::from(calls::PlayerState),
        HostCall::from(calls::UndergroundBiomeAt {
            positions: vec![[0, 0, 0]],
        }),
        HostCall::from(calls::ClientWorldStateWrite {
            scope: ClientStorageScope::Pack,
            path: "w.pmc".into(),
            select: mod_api::ClientStateSelect::All,
            kinds: None,
            envelopes: None,
        }),
        HostCall::from(calls::ClientWorldEventsBegin {
            scope: ClientStorageScope::Pack,
            path: "e.pmc".into(),
            envelopes: None,
        }),
        HostCall::from(calls::ClientFrameCapture {
            source: mod_api::ClientCaptureSource::Scene,
            size: None,
            when: mod_api::ClientCaptureWhen::Next,
            advance: true,
            into: mod_api::ClientCaptureInto::Media(1),
        }),
        HostCall::from(calls::ClientClockSet { step: None }),
        HostCall::from(calls::ClientClockAdvance),
        HostCall::from(calls::ClientAudioTap {
            sample_rate: 48_000,
            channels: 2,
            into: mod_api::ClientAudioInto::Media(1),
        }),
        HostCall::from(calls::ClientStorageGetMany {
            scope: ClientStorageScope::World,
            keys: vec!["map:k".into()],
        }),
        HostCall::from(calls::ClientStorageSetMany {
            scope: ClientStorageScope::World,
            entries: vec![("map:k".into(), Some(mod_api::ByteBuf::from(vec![1])))],
        }),
    ];
    for call in world_calls {
        let name = format!("{call:?}");
        match handle_host_call(&mut data, call) {
            HostRet::Err(mod_api::HostError {
                detail: message, ..
            }) => assert!(
                message.contains("world") && !message.contains("simulation"),
                "{name}: {message}"
            ),
            other => panic!("{name} answered on the shell: {other:?}"),
        }
    }
    assert!(!dir.join("pack").exists(), "nothing reached a disk");

    assert_eq!(
        handle_host_call(&mut data, HostCall::from(calls::ClientContext)),
        HostRet::ClientContext(ClientContext::Shell)
    );
    for call in [
        HostCall::from(calls::ClientUiStateSet {
            key: "map:title".into(),
            value: mod_api::GuiValue::Str("t".into()),
        }),
        HostCall::from(calls::ClientTextMeasure {
            text: "t".into(),
            scale: 1,
        }),
        HostCall::from(calls::ClientGuiOpen {
            kind_key: "map:browser".into(),
        }),
        HostCall::from(calls::ClientPresentationState),
        HostCall::from(calls::ClientEngineFacts),
        HostCall::from(calls::ClientPacks),
        HostCall::from(calls::ClientWallClock),
        HostCall::from(calls::ClientFileStat {
            scope: ClientStorageScope::Pack,
            path: "a".into(),
        }),
        HostCall::from(calls::ClientMediaState { media: 1 }),
        HostCall::from(calls::ClientStorageSetMany {
            scope: ClientStorageScope::Pack,
            entries: vec![("map:k".into(), Some(mod_api::ByteBuf::from(vec![1])))],
        }),
    ] {
        let name = format!("{call:?}");
        assert!(
            !matches!(handle_host_call(&mut data, call), HostRet::Err(_)),
            "{name} is refused on the shell"
        );
    }
    drop(data);
}

/// The PACK bucket follows the mod, not the world: what a session filed there
/// — during a presentation too, which keeps its hands off the WORLD bucket —
/// is what the next session reads, even one on the shell with no world at all.
#[test]
fn pack_storage_survives_across_sessions_and_presentations() {
    use mod_api::ClientStorageScope::{Pack, World};
    let dir = TestScratchDir::new("client-calls-pack-storage");
    let set = |data: &mut ModStoreData, scope, value: u8| {
        handle_host_call(
            data,
            HostCall::from(calls::ClientStorageSetMany {
                scope,
                entries: vec![(
                    "map:project".into(),
                    Some(mod_api::ByteBuf::from(vec![value])),
                )],
            }),
        )
    };
    let holds = |value: Option<u8>| {
        HostRet::ClientStorageValues(vec![value.map(|v| mod_api::ByteBuf::from(vec![v]))])
    };

    let mut world = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(crate::modding::client::ClientBuckets {
            world: Some(dir.join("world")),
            pack: dir.join("pack"),
        }),
    );
    let presented = world.client.as_ref().unwrap().presented.clone();
    let queued = |ret| matches!(ret, HostRet::ClientStorageWrite(_));
    assert!(queued(set(&mut world, Pack, 1)));
    presented.lock().context = mod_api::ClientContext::Presentation {
        owner: "map".into(),
    };
    assert!(matches!(
        set(&mut world, World, 9),
        HostRet::Err(mod_api::HostError {
            code: mod_api::ErrorCode::Refused,
            ..
        })
    ));
    assert!(
        queued(set(&mut world, Pack, 2)),
        "a presentation keeps the pack bucket"
    );
    drop(world);

    let mut shell = shell_data(&dir);
    assert_eq!(
        handle_host_call(
            &mut shell,
            HostCall::from(calls::ClientStorageGetMany {
                scope: Pack,
                keys: vec!["map:project".into()],
            }),
        ),
        holds(Some(2)),
        "the next session, on the shell, reads it"
    );
    let HostRet::U64(ticket) = handle_host_call(
        &mut shell,
        HostCall::from(calls::ClientStorageReadBegin {
            scope: Pack,
            keys: vec!["map:project".into()],
        }),
    ) else {
        panic!("a pack read begins on the shell");
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match handle_host_call(
            &mut shell,
            HostCall::from(calls::ClientStorageReadPoll {
                scope: Pack,
                ticket,
            }),
        ) {
            HostRet::ClientStorageRead(None) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the read never landed"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            HostRet::ClientStorageRead(Some(values)) => {
                assert_eq!(values, vec![Some(mod_api::ByteBuf::from(vec![2]))]);
                break;
            }
            other => panic!("poll answered {other:?}"),
        }
    }
    drop(shell);
    let reopened = |dir: &std::path::Path| {
        ModStoreData::new_for_side(
            "map",
            7,
            RuntimeSide::Client,
            Some(crate::modding::client::ClientBuckets {
                world: Some(dir.join("world")),
                pack: dir.join("pack"),
            }),
        )
    };
    assert_eq!(
        handle_host_call(
            &mut reopened(&dir),
            HostCall::from(calls::ClientStorageGetMany {
                scope: World,
                keys: vec!["map:project".into()],
            }),
        ),
        holds(None),
        "the presentation's world write never landed"
    );
}

/// A client's `Raycast` is the sim's ray cast against the REPLICA: the
/// switchboard sends a client instance's block-domain ray to the client
/// handler, and it is validated the same way.
#[test]
fn a_client_raycast_answers_from_the_replica() {
    let scratch = TestScratchDir::new("client-calls-raycast");
    let mut data = client_data(&scratch);
    let mut world = crate::world::ReplicaWorld::new(0, 0);
    let sp = petramond_world::chunk::SectionPos::new(0, 4, 0);
    world.insert_section_for_test(sp, petramond_world::section::Section::new(0, 4, 0));
    assert!(world.set_block_world(3, 64, 5, petramond_world::block::Block::Stone));
    let ray = |max| {
        HostCall::from(calls::Raycast {
            from: [3.5, 64.5, 0.5],
            dir: [0.0, 0.0, 1.0],
            max,
            filter: mod_api::RayFilter::Collidable,
        })
    };
    let hit = super::client_scope::enter(&world, || handle_host_call(&mut data, ray(16.0)));
    let HostRet::Raycast(Some(hit)) = hit else {
        panic!("a hit on the replica's stone, got {hit:?}");
    };
    assert_eq!(hit.block, [3, 64, 5]);
    assert!(matches!(
        super::client_scope::enter(&world, || handle_host_call(&mut data, ray(65.0))),
        HostRet::Err(_)
    ));
}

/// A media open is checked against the machine's own answer, never against a
/// size of the engine's: a file only a mod's pushes feed opens at any size.
/// A second writer of the same path is refused, and a wrong-sized push is
/// the mod's bug.
#[test]
fn a_push_only_media_file_of_any_size_opens_and_its_path_is_its_own() {
    use mod_api::ClientStorageScope::Pack;
    let scratch = TestScratchDir::new("client-calls-media-open");
    let mut data = client_data(&scratch);
    let open = |size: u32| {
        HostCall::from(calls::ClientMediaOpen {
            scope: Pack,
            path: "videos/huge.mkv".into(),
            container: "matroska".into(),
            video: Some(mod_api::ClientMediaVideo {
                codec: "ffv1".into(),
                width: size,
                height: size,
                fps: [1, 1],
                options: Vec::new(),
            }),
            audio: None,
            options: Vec::new(),
        })
    };
    assert!(
        matches!(
            handle_host_call(&mut data, open(8)),
            HostRet::Err(mod_api::HostError {
                code: mod_api::ErrorCode::Refused,
                ..
            })
        ),
        "nothing opens before the machine was asked"
    );
    data.client
        .as_ref()
        .unwrap()
        .media
        .publish_encoders(mod_api::ClientMediaCapabilities {
            encoder: Some("ffmpeg".into()),
            containers: vec!["matroska".into()],
            video_codecs: vec!["ffv1".into()],
            audio_codecs: Vec::new(),
        });
    let HostRet::Ticket(media) = handle_host_call(&mut data, open(100_000)) else {
        panic!("a push-only file was refused for its size");
    };
    assert!(
        matches!(
            handle_host_call(&mut data, open(8)),
            HostRet::Err(mod_api::HostError {
                code: mod_api::ErrorCode::Refused,
                ..
            })
        ),
        "the path is being written"
    );
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ClientMediaPushFrame {
                media,
                rgba: vec![0; 4],
            })
        ),
        HostRet::Err(_)
    ));
    assert_eq!(
        handle_host_call(&mut data, HostCall::from(calls::ClientMediaAbort { media })),
        HostRet::Unit
    );
    let HostRet::ClientMediaState(Some(state)) =
        handle_host_call(&mut data, HostCall::from(calls::ClientMediaState { media }))
    else {
        panic!("the file's state is gone");
    };
    assert_eq!(state.failure, Some(mod_api::ClientMediaFailure::Aborted));
    assert!(
        matches!(handle_host_call(&mut data, open(8)), HostRet::Ticket(_)),
        "an aborted file lets go of its path"
    );
}

/// A folder the player chose is a place the mod writes files into by slot,
/// never by path: nothing before the choice, one picker at a time, a cancel
/// keeps what was chosen, and the choice outlives the instance that asked.
#[test]
fn a_chosen_folder_takes_files_only_once_the_player_picked_it() {
    use crate::modding::client::files::folders::{install_chooser, FolderRequest};
    use mod_api::{ClientFileAnswer, ClientStorageScope::Chosen, ErrorCode};
    use std::sync::{Arc, Mutex};

    let refused = |ret: &HostRet| matches!(ret, HostRet::Err(e) if e.code == ErrorCode::Refused);
    let scratch = TestScratchDir::new("client-calls-chosen-folder");
    let picked = scratch.join("videos");
    std::fs::create_dir_all(&picked).unwrap();
    let asked: Arc<Mutex<Vec<FolderRequest>>> = Arc::default();
    let queue = Arc::clone(&asked);
    install_chooser(Some(Arc::new(move |request| {
        queue.lock().unwrap().push(request)
    })));
    let answer = |data: &mut ModStoreData, ticket: u64| {
        let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
        loop {
            match handle_host_call(data, HostCall::from(calls::ClientFilePoll { ticket })) {
                HostRet::ClientFilePolled(Some(answer)) => return answer,
                HostRet::ClientFilePolled(None) => {}
                other => panic!("the poll answered {other:?}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ticket {ticket} never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    let choose = |data: &mut ModStoreData| {
        handle_host_call(
            data,
            HostCall::from(calls::ClientFolderChoose {
                folder: 0,
                title: "Export to".into(),
            }),
        )
    };
    let state = |data: &mut ModStoreData| {
        handle_host_call(data, HostCall::from(calls::ClientFolderState { folder: 0 }))
    };
    let append = || {
        HostCall::from(calls::ClientFileAppend {
            scope: Chosen(0),
            path: "clip.bin".into(),
            bytes: vec![1, 2, 3],
        })
    };

    let mut data = client_data(&scratch);
    assert!(
        refused(&handle_host_call(&mut data, append())),
        "nothing is written before the player chose"
    );
    assert_eq!(state(&mut data), HostRet::ClientFolder(None));

    let HostRet::Ticket(ticket) = choose(&mut data) else {
        panic!("the picker did not open");
    };
    assert!(refused(&choose(&mut data)), "one picker at a time");
    let request = asked.lock().unwrap().pop().unwrap();
    request.done.answer(Some(picked.clone()));
    let ClientFileAnswer::Folder(Some(info)) = answer(&mut data, ticket) else {
        panic!("the choice was not answered");
    };
    assert!(info.label.ends_with("videos"), "{}", info.label);

    let HostRet::Ticket(write) = handle_host_call(&mut data, append()) else {
        panic!("the chosen folder refused a file");
    };
    assert!(matches!(
        answer(&mut data, write),
        ClientFileAnswer::Done { .. }
    ));
    assert_eq!(std::fs::read(picked.join("clip.bin")).unwrap(), [1, 2, 3]);

    let HostRet::Ticket(ticket) = choose(&mut data) else {
        panic!("the picker did not open again");
    };
    let request = asked.lock().unwrap().pop().unwrap();
    assert_eq!(
        request.start.as_deref(),
        Some(picked.as_path()),
        "it opens where it was"
    );
    drop(request); // a picker that never answers cancels
    assert_eq!(answer(&mut data, ticket), ClientFileAnswer::Folder(None));

    let mut later = client_data(&scratch);
    assert_eq!(
        state(&mut later),
        HostRet::ClientFolder(Some(info)),
        "a cancel keeps the choice, and a new instance remembers it"
    );
    assert!(matches!(
        handle_host_call(
            &mut later,
            HostCall::from(calls::ClientStorageGetMany {
                scope: Chosen(0),
                keys: vec!["weathertest:k".into()],
            })
        ),
        HostRet::Err(_)
    ));
    std::fs::remove_dir_all(&picked).unwrap();
    assert_eq!(
        state(&mut later),
        HostRet::ClientFolder(None),
        "a folder that is gone is no choice"
    );
    install_chooser(None);
}
