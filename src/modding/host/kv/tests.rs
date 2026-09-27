use mod_api::calls;
use mod_api::{HostCall, HostRet};

use crate::events::tick::TickEvents;
use crate::events::{PostQueue, RosterRefs, SimCtx};
use crate::modding::host::guards::KV_MAX_VALUE_BYTES;
use crate::modding::host::{handle_host_call, ModStoreData};
use crate::modding::scope;
use crate::world::ServerWorld;

#[test]
fn sparse_find_is_sorted_and_never_fabricates_unloaded_cells() {
    let mut data = ModStoreData::new("fixture", 1);
    with_ctx(|| {
        for pos in [[3, 65, 1], [1, 64, 2], [2, 64, 1]] {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::SectionKvSet {
                        pos,
                        key: "fixture:marker".into(),
                        value: vec![1],
                    })
                ),
                HostRet::Bool(true)
            );
        }
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::from(calls::SectionKvFind {
                    section: [0, 4, 0],
                    key: "fixture:marker".into(),
                })
            ),
            HostRet::FoundBlocks(Some(vec![[2, 64, 1], [1, 64, 2], [3, 65, 1]]))
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::from(calls::SectionKvFind {
                    section: [-2, -3, 4],
                    key: "fixture:marker".into(),
                })
            ),
            HostRet::FoundBlocks(None)
        );
        scope::with_active(|ctx| {
            ctx.world
                .mark_overlay_in_flight_for_test(petramond_world::chunk::SectionPos::new(0, 4, 0))
        });
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::from(calls::SectionKvFind {
                    section: [0, 4, 0],
                    key: "fixture:marker".into(),
                })
            ),
            HostRet::FoundBlocks(None),
            "generated markers stay hidden until saved terrain is final"
        );
    });
}

fn with_ctx(f: impl FnOnce()) {
    let mut world = ServerWorld::new(1, 1);
    let mut c = petramond_world::chunk::Chunk::new(0, 0);
    for z in 0..petramond_world::chunk::CHUNK_SZ {
        for x in 0..petramond_world::chunk::CHUNK_SX {
            c.set_block(x, 64, z, petramond_world::block::Block::Stone);
        }
    }
    world.insert_chunk_for_test(petramond_world::chunk::ChunkPos::new(0, 0), c);
    let mut nobody = RosterRefs::empty();
    let mut feed = TickEvents::default();
    let mut queue = PostQueue::default();
    let mut ctx = SimCtx {
        world: &mut world,
        actor: None,
        players: &mut nobody,
        feed: &mut feed,
        queue: &mut queue,
    };
    scope::enter(&mut ctx, f);
}

#[test]
fn kv_writes_enforce_own_namespace_and_reads_cross() {
    let mut alpha = ModStoreData::new("alpha", 1);
    let mut beta = ModStoreData::new("beta", 1);
    with_ctx(|| {
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::from(calls::WorldKvSet {
                    key: "alpha:x".into(),
                    value: vec![7],
                }),
            ),
            HostRet::Unit
        );
        assert_eq!(
            handle_host_call(
                &mut beta,
                HostCall::from(calls::WorldKvSet {
                    key: "petramond:time".into(),
                    value: vec![1],
                }),
            ),
            HostRet::Unit
        );
        assert!(matches!(
            handle_host_call(
                &mut beta,
                HostCall::from(calls::WorldKvSet {
                    key: "alpha:x".into(),
                    value: vec![9],
                }),
            ),
            HostRet::Err(_)
        ));
        for bad in ["x", "alpha:", "petramond:", "alphax:y", "beta"] {
            assert!(
                matches!(
                    handle_host_call(
                        &mut beta,
                        HostCall::from(calls::WorldKvSet {
                            key: bad.into(),
                            value: vec![1],
                        }),
                    ),
                    HostRet::Err(_)
                ),
                "write with key '{bad}' must be rejected"
            );
        }
        assert_eq!(
            handle_host_call(
                &mut beta,
                HostCall::from(calls::WorldKvGet {
                    key: "alpha:x".into(),
                }),
            ),
            HostRet::Bytes(Some(vec![7]))
        );
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::from(calls::WorldKvGet {
                    key: "petramond:time".into(),
                }),
            ),
            HostRet::Bytes(Some(vec![1]))
        );
        assert!(matches!(
            handle_host_call(
                &mut beta,
                HostCall::from(calls::WorldKvDelete {
                    key: "alpha:x".into(),
                }),
            ),
            HostRet::Err(_)
        ));
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::from(calls::WorldKvDelete {
                    key: "alpha:x".into(),
                }),
            ),
            HostRet::Bool(true)
        );
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::from(calls::WorldKvSet {
                    key: "alpha:big".into(),
                    value: vec![0; KV_MAX_VALUE_BYTES + 1],
                }),
            ),
            HostRet::Err(_)
        ));
    });
    assert!(matches!(
        handle_host_call(
            &mut alpha,
            HostCall::from(calls::WorldKvGet {
                key: "alpha:x".into(),
            }),
        ),
        HostRet::Err(_)
    ));
}

#[test]
fn section_kv_caps_distinct_keys_per_cell() {
    use crate::modding::host::guards::CELL_KV_MAX_KEYS;
    let mut alpha = ModStoreData::new("alpha", 1);
    with_ctx(|| {
        let pos = [2, 65, 2];
        let set = |m: &mut ModStoreData, key: String| {
            handle_host_call(
                m,
                HostCall::from(calls::SectionKvSet {
                    pos,
                    key,
                    value: vec![1],
                }),
            )
        };
        for i in 0..CELL_KV_MAX_KEYS {
            assert_eq!(
                set(&mut alpha, format!("alpha:k{i}")),
                HostRet::Bool(true),
                "key {i} fits under the cap"
            );
        }
        assert!(
            matches!(
                set(&mut alpha, "alpha:one_too_many".into()),
                HostRet::Err(_)
            ),
            "a new key beyond the cap is rejected"
        );
        assert_eq!(
            set(&mut alpha, "alpha:k0".into()),
            HostRet::Bool(true),
            "overwriting an existing key at the cap passes"
        );
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::from(calls::SectionKvDelete {
                    pos,
                    key: "alpha:k1".into(),
                }),
            ),
            HostRet::Bool(true)
        );
        assert_eq!(
            set(&mut alpha, "alpha:one_too_many".into()),
            HostRet::Bool(true),
            "a removal frees a slot"
        );
    });
}
