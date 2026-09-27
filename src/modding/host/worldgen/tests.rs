use mod_api::calls;
use mod_api::{HostCall, HostRet};

use crate::modding::host::{handle_host_call, ModStoreData, Phase, Registration};

#[test]
fn gen_registrations_gate_on_the_init_window() {
    let mut data = ModStoreData::new("alpha", 1);
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::RegisterWorldgenFeature {
                feature_id: 1,
                stage: mod_api::WorldgenStage::Trees,
                filter: Default::default(),
            }),
        ),
        HostRet::Unit
    );
    assert!(matches!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::RegisterWorldgenFeature {
                feature_id: 2,
                stage: mod_api::WorldgenStage::Climate,
                filter: Default::default(),
            }),
        ),
        HostRet::Err(_)
    ));
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::RegisterStageReplacement {
                stage: mod_api::WorldgenStage::Terrain,
                callback_id: 3,
            }),
        ),
        HostRet::Unit
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::RegisterGenerator { callback_id: 4 })
        ),
        HostRet::Unit
    );
    assert_eq!(data.stats.registered, 3);
    assert!(data.pending.iter().all(Registration::is_gen));

    data.phase = Phase::Run;
    for call in [
        HostCall::from(calls::RegisterWorldgenFeature {
            feature_id: 1,
            stage: mod_api::WorldgenStage::Trees,
            filter: Default::default(),
        }),
        HostCall::from(calls::RegisterStageReplacement {
            stage: mod_api::WorldgenStage::Terrain,
            callback_id: 3,
        }),
        HostCall::from(calls::RegisterGenerator { callback_id: 4 }),
    ] {
        assert!(matches!(handle_host_call(&mut data, call), HostRet::Err(_)));
    }
    assert_eq!(data.stats.rejected_registrations, 4);
}

#[test]
fn inverted_feature_bounds_are_rejected_at_registration() {
    let mut data = ModStoreData::new("alpha", 1);
    for filter in [
        mod_api::GenFeatureFilter::y_band(10, -10),
        mod_api::GenFeatureFilter::surface_band(2, 1),
    ] {
        assert!(matches!(
            handle_host_call(
                &mut data,
                HostCall::from(calls::RegisterWorldgenFeature {
                    feature_id: 1,
                    stage: mod_api::WorldgenStage::Trees,
                    filter,
                }),
            ),
            HostRet::Err(_)
        ));
    }
    assert_eq!(data.stats.registered, 0);
    assert_eq!(data.stats.rejected_registrations, 2);
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::RegisterWorldgenFeature {
                feature_id: 1,
                stage: mod_api::WorldgenStage::Trees,
                filter: mod_api::GenFeatureFilter::y_band(-10, 10).without_blocks(),
            }),
        ),
        HostRet::Unit,
        "an ordered band registers"
    );
}

/// Name to id, then biome per cell. Both have to work at generate time, not just init, and the
/// batch reply keeps request order.
#[test]
fn underground_biome_calls_answer_outside_the_init_window() {
    let mut data = ModStoreData::new("alpha", 0x312);
    data.phase = Phase::Run;

    let id = match handle_host_call(
        &mut data,
        HostCall::from(calls::ResolveUndergroundBiome {
            key: "petramond:stone".into(),
        }),
    ) {
        HostRet::MaybeByte(id) => id.expect("a shipped row resolves"),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        petramond_worldgen::data::underground::table().name(id),
        Some("petramond:stone"),
        "name -> id -> name round-trips"
    );
    assert_eq!(
        handle_host_call(
            &mut data,
            HostCall::from(calls::ResolveUndergroundBiome {
                key: "nope:nothing".into(),
            }),
        ),
        HostRet::MaybeByte(None),
        "an unknown name degrades, it is not an error"
    );

    let positions = vec![[0, -20, 0], [244, 0, 244], [-500, -60, 300]];
    let ids = match handle_host_call(
        &mut data,
        HostCall::from(calls::UndergroundBiomeAt {
            positions: positions.clone(),
        }),
    ) {
        HostRet::UndergroundBiomes(ids) => ids,
        other => panic!("{other:?}"),
    };
    assert_eq!(ids.len(), positions.len(), "reply parallels the request");
    for (p, got) in positions.iter().zip(&ids) {
        assert_eq!(
            *got,
            petramond_worldgen::underground_biomes_at(0x312, &[*p])[0],
            "the ABI answer is the engine's own at {p:?}"
        );
    }

    let extremes = vec![
        [i32::MIN, i32::MIN, i32::MIN],
        [i32::MAX, i32::MAX, i32::MAX],
        [i32::MIN, 0, i32::MAX],
    ];
    match handle_host_call(
        &mut data,
        HostCall::from(calls::UndergroundBiomeAt {
            positions: extremes.clone(),
        }),
    ) {
        HostRet::UndergroundBiomes(ids) => assert_eq!(ids.len(), extremes.len()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_surface_biome_query_batches_across_tiles_in_any_order() {
    let mut data = ModStoreData::new("alpha", 0x312);
    data.phase = Phase::Run;

    let columns = vec![[3, 5], [900, -1100], [-40, 12], [900, -1100], [-1500, 700]];
    let ids = match handle_host_call(
        &mut data,
        HostCall::from(calls::SurfaceBiomeAt {
            columns: columns.clone(),
        }),
    ) {
        HostRet::SurfaceBiomes(ids) => ids,
        other => panic!("{other:?}"),
    };
    assert_eq!(ids.len(), columns.len(), "reply parallels the request");
    for (column, got) in columns.iter().zip(&ids) {
        assert_eq!(
            petramond_world::biome::Biome::from_id(*got).id(),
            *got,
            "every column answers a real biome id; {column:?} answered {got}"
        );
        let alone = match handle_host_call(
            &mut data,
            HostCall::from(calls::SurfaceBiomeAt {
                columns: vec![*column],
            }),
        ) {
            HostRet::SurfaceBiomes(ids) => ids[0],
            other => panic!("{other:?}"),
        };
        assert_eq!(*got, alone, "batched answer differs at {column:?}");
    }

    match handle_host_call(
        &mut data,
        HostCall::from(calls::SurfaceBiomeAt {
            columns: vec![[i32::MIN, i32::MAX], [i32::MAX, i32::MIN]],
        }),
    ) {
        HostRet::SurfaceBiomes(ids) => assert_eq!(ids.len(), 2),
        other => panic!("{other:?}"),
    }
}
