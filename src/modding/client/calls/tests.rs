use mod_api::{HostCall, HostRet, RuntimeSide};

use crate::modding::host::{handle_host_call, ModStoreData};

fn client_data(tag: &str) -> ModStoreData {
    ModStoreData::new_for_side(
        "weathertest",
        7,
        RuntimeSide::Client,
        Some(std::env::temp_dir().join(format!("petramond-client-calls-{tag}"))),
    )
}

/// The weather-era client calls: unknown keys are FORGIVING `false`
/// (a disabled pack is not a protocol break), malformed values are hard
/// errors, and the env-param read is capped at the GPU slot budget.
#[test]
fn weather_era_client_calls_validate_and_forgive() {
    let mut data = client_data("weather-era");
    // Unknown bundle key / unknown sound key: forgiving false.
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientAmbientSet {
                key: "nope:rain".into(),
                intensity: 1.0,
                wind: [0.0, 0.0],
            },
        ),
        HostRet::Bool(false)
    );
    // A real bundle that is NOT ambient (the engine water splash burst):
    // also forgiving false.
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientAmbientSet {
                key: crate::particle_emitters::WATER_SPLASH_KEY.into(),
                intensity: 1.0,
                wind: [0.0, 0.0],
            },
        ),
        HostRet::Bool(false)
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientLoopSet {
                key: "nope:loop".into(),
                gain: 1.0,
            },
        ),
        HostRet::Bool(false)
    );
    // Non-finite / out-of-envelope values are hard errors.
    for bad in [
        HostCall::ClientAmbientSet {
            key: "m:x".into(),
            intensity: f32::NAN,
            wind: [0.0, 0.0],
        },
        HostCall::ClientAmbientSet {
            key: "m:x".into(),
            intensity: 1.0,
            wind: [65.0, 0.0],
        },
        HostCall::ClientLoopSet {
            key: "m:x".into(),
            gain: f32::INFINITY,
        },
        HostCall::ClientMoodSet {
            darken: f32::NAN,
            desaturate: 0.0,
        },
    ] {
        let ret = handle_host_call(&mut data, bad.clone());
        assert!(
            matches!(ret, HostRet::Error(_)),
            "malformed values must be a hard error: {bad:?} -> {ret:?}"
        );
    }
    // The mood clamps into its subtle envelope and always succeeds.
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientMoodSet {
                darken: 9.0,
                desaturate: -3.0,
            },
        ),
        HostRet::Bool(true)
    );
    assert_eq!(data.client.as_ref().unwrap().mood, [0.5, 0.0]);
    // Env-param reads cap at the 16-slot GPU budget.
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::ClientEnvParams {
                keys: (0..17).map(|i| format!("m:k{i}")).collect(),
            },
        ),
        HostRet::Error(_)
    ));
}

/// The weather-era SERVER calls are rejected on a client instance by the
/// capability gate, like every sim-facing call.
#[test]
fn weather_era_server_calls_stay_server_side() {
    let mut data = client_data("server-side");
    for call in [
        HostCall::BiomeAt { pos: [0, 0] },
        HostCall::SurfaceYAt { pos: [0, 0] },
        HostCall::Players,
    ] {
        assert!(
            matches!(handle_host_call(&mut data, call), HostRet::Error(_)),
            "sim queries must be rejected on client instances"
        );
    }
}

#[test]
fn client_instances_are_capability_isolated_and_namespace_their_state() {
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(std::env::temp_dir().join("petramond-unused-client-mod-test")),
    );
    assert_eq!(
        handle_host_call(&mut data, HostCall::RuntimeSide),
        HostRet::RuntimeSide(RuntimeSide::Client)
    );
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::RegisterTickSystem {
                stage: mod_api::Stage::Mobs,
                attach: mod_api::AttachSide::After,
                priority: 0,
                system_id: 1,
            }
        ),
        HostRet::Error(_)
    ));
    assert!(data.pending.is_empty());
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::ClientUiStateSet {
                key: "other:value".into(),
                value: mod_api::GuiValue::I32(1),
            }
        ),
        HostRet::Error(_)
    ));
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientUiStateSet {
                key: "map:value".into(),
                value: mod_api::GuiValue::I32(2),
            }
        ),
        HostRet::Unit
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientUiStateGet {
                key: "map:value".into(),
            }
        ),
        HostRet::GuiValue(Some(mod_api::GuiValue::I32(2)))
    );

    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientImageSet {
                key: "map:tile".into(),
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 255],
            },
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
            HostCall::ClientCanvasSceneSet {
                canvas_key: "map:canvas".into(),
                elements: elements.clone(),
            },
        ),
        HostRet::Unit
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientCanvasViewSet {
                canvas_key: "map:canvas".into(),
                offset: [12.0, -7.0],
            },
        ),
        HostRet::Unit
    );
    let client = data.client.as_ref().unwrap();
    let scene = client.canvas_scenes.get("map:canvas").unwrap();
    assert_eq!(scene.elements, elements);
    assert_eq!(scene.offset, [12.0, -7.0]);
    assert_eq!(client.images["map:tile"].revision, image_revision);
}

#[test]
fn client_image_blit_mutates_in_place_and_validates_bounds() {
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(std::env::temp_dir().join("petramond-unused-client-blit-test")),
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientImageSet {
                key: "map:tile".into(),
                width: 2,
                height: 2,
                rgba: vec![0; 16],
            },
        ),
        HostRet::Unit
    );
    let revision = data.client.as_ref().unwrap().images["map:tile"].revision;
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ClientImageBlit {
                key: "map:tile".into(),
                origin: [1, 1],
                size: [1, 1],
                rgba: vec![9, 8, 7, 255],
            },
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
            HostCall::ClientImageBlit {
                key: "map:tile".into(),
                origin: [0, 0],
                size: [1, 1],
                rgba: vec![1, 1, 1, 255],
            },
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
        HostCall::ClientImageDrawTexts {
            key: "map:tile".into(),
            runs: vec![mod_api::ClientTextRun {
                text: "x".into(),
                position: [0, 0],
                scale: 1,
                color: [255, 255, 255, 255],
            }],
        },
    );
    let image = &data.client.as_ref().unwrap().images["map:tile"];
    assert!(
        image.recent_blits.is_empty(),
        "text draws break the partial chain (no rect is tracked for them)"
    );

    for bad in [
        // out of bounds
        HostCall::ClientImageBlit {
            key: "map:tile".into(),
            origin: [2, 0],
            size: [1, 1],
            rgba: vec![0; 4],
        },
        // byte count mismatch
        HostCall::ClientImageBlit {
            key: "map:tile".into(),
            origin: [0, 0],
            size: [1, 1],
            rgba: vec![0; 3],
        },
        // never published
        HostCall::ClientImageBlit {
            key: "map:none".into(),
            origin: [0, 0],
            size: [1, 1],
            rgba: vec![0; 4],
        },
        // foreign namespace
        HostCall::ClientImageBlit {
            key: "other:tile".into(),
            origin: [0, 0],
            size: [1, 1],
            rgba: vec![0; 4],
        },
    ] {
        assert!(matches!(
            handle_host_call(&mut data, bad),
            HostRet::Error(_)
        ));
    }
}

#[test]
fn client_surface_columns_gate_on_revision_and_pack_cells() {
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(std::env::temp_dir().join("petramond-unused-client-surface-test")),
    );
    let mut world = crate::world::World::new(0, 0);
    let sp = crate::chunk::SectionPos::new(0, 4, 0);
    world.insert_section_for_test(sp, crate::section::Section::new(0, 4, 0));
    assert!(world.set_block_world(3, 64, 5, crate::block::Block::Stone));

    let query = |revision| HostCall::ClientSurfaceColumns {
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
    assert!(world.set_block_world(3, 64, 5, crate::block::Block::Dirt));
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
    let mut data = ModStoreData::new_for_side(
        "map",
        7,
        RuntimeSide::Client,
        Some(std::env::temp_dir().join("petramond-unused-client-blocks-test")),
    );
    let mut world = crate::world::World::new(0, 0);
    let sp = crate::chunk::SectionPos::new(0, 4, 0);
    world.insert_section_for_test(sp, crate::section::Section::new(0, 4, 0));
    assert!(world.set_block_world(3, 64, 5, crate::block::Block::Stone));

    let query = || HostCall::ClientBlocksAt {
        positions: vec![[3, 64, 5], [3, 65, 5], [150, 64, 5]],
    };
    let HostRet::Blocks(blocks) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query()))
    else {
        panic!("blocks reply expected");
    };
    assert_eq!(blocks[0], Some(mod_api::BlockId(crate::block::Block::Stone.id())));
    assert_eq!(blocks[1], Some(mod_api::BlockId(crate::block::Block::Air.id())));
    assert_eq!(blocks[2], None, "an unloaded section reads None");

    // A section whose streamed content is not final reads None — the same
    // "state frozen, retry later" contract as the server-side mod reads.
    world.mark_overlay_in_flight_for_test(sp);
    let HostRet::Blocks(blocks) =
        super::client_scope::enter(&world, || handle_host_call(&mut data, query()))
    else {
        panic!("blocks reply expected");
    };
    assert_eq!(blocks[0], None, "an in-flight overlay leaked a replica read");

    // Registry-only queries are legal on client instances (a client mod
    // interpreting block ids has to resolve the names and tag sets it
    // compares to).
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::ResolveBlock {
                name: "petramond:stone".into()
            }
        ),
        HostRet::Block(Some(mod_api::BlockId(crate::block::Block::Stone.id())))
    );
    let HostRet::BlockList(leaves) = handle_host_call(
        &mut data,
        HostCall::BlocksByTag {
            tag: "petramond:leaves".into(),
        },
    ) else {
        panic!("block list expected");
    };
    assert!(!leaves.is_empty());

    // The batch bound is enforced.
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::ClientBlocksAt {
                positions: vec![[0, 0, 0]; 513],
            }
        ),
        HostRet::Error(_)
    ));
}
