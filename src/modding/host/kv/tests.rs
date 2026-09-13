use mod_api::{HostCall, HostRet};

use crate::events::tick::TickEvents;
use crate::events::{PostQueue, SimCtx};
use crate::modding::host::guards::KV_MAX_VALUE_BYTES;
use crate::modding::host::{handle_host_call, ModStoreData};
use crate::modding::scope;
use crate::player::Player;
use crate::world::World;
use petramond_math::world_pos::WorldPos;

#[test]
fn sparse_find_is_sorted_and_never_fabricates_unloaded_cells() {
    let mut data = ModStoreData::new("fixture", 1);
    with_ctx(|| {
        for pos in [[3, 65, 1], [1, 64, 2], [2, 64, 1]] {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::SectionKvSet {
                        pos,
                        key: "fixture:marker".into(),
                        value: vec![1],
                    }
                ),
                HostRet::Bool(true)
            );
        }
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::SectionKvFind {
                    section: [0, 4, 0],
                    key: "fixture:marker".into(),
                }
            ),
            HostRet::FoundBlocks(Some(vec![[2, 64, 1], [1, 64, 2], [3, 65, 1]]))
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::SectionKvFind {
                    section: [-2, -3, 4],
                    key: "fixture:marker".into(),
                }
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
                HostCall::SectionKvFind {
                    section: [0, 4, 0],
                    key: "fixture:marker".into(),
                }
            ),
            HostRet::FoundBlocks(None),
            "generated markers stay hidden until saved terrain is final"
        );
    });
}

/// Run `f` with a live SimCtx published, as if inside a guest dispatch.
/// The world gets one flat-floored loaded chunk so section-cell KV writes
/// have a writable target.
fn with_ctx(f: impl FnOnce()) {
    let mut world = World::new(1, 1);
    let mut c = petramond_world::chunk::Chunk::new(0, 0);
    for z in 0..petramond_world::chunk::CHUNK_SZ {
        for x in 0..petramond_world::chunk::CHUNK_SX {
            c.set_block(x, 64, z, petramond_world::block::Block::Stone);
        }
    }
    world.insert_chunk_for_test(petramond_world::chunk::ChunkPos::new(0, 0), c);
    let mut player = Player::new(WorldPos::new(0.0, 80.0, 0.0));
    let mut feed = TickEvents::default();
    let mut queue = PostQueue::default();
    let mut gui = petramond_world::gui_state::empty_gui_state();
    let mut ctx = SimCtx {
        world: &mut world,
        player: &mut player,
        gui_state: &mut gui,
        feed: &mut feed,
        queue: &mut queue,
    };
    scope::enter(&mut ctx, f);
}

/// The KV namespace contract: writes must carry the CALLER's own
/// `mod_id:` prefix or an engine-owned `petramond:` key (foreign and bare keys
/// are rejected with an error), while reads may cross namespaces — that
/// asymmetry IS the cross-mod interop surface. Size caps reject oversized
/// values.
#[test]
fn kv_writes_enforce_own_namespace_and_reads_cross() {
    let mut alpha = ModStoreData::new("alpha", 1);
    let mut beta = ModStoreData::new("beta", 1);
    with_ctx(|| {
        // Own-prefix write lands.
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::WorldKvSet {
                    key: "alpha:x".into(),
                    value: vec![7],
                },
            ),
            HostRet::Unit
        );
        // Engine-owned public surfaces are intentionally writable.
        assert_eq!(
            handle_host_call(
                &mut beta,
                HostCall::WorldKvSet {
                    key: "petramond:time".into(),
                    value: vec![1],
                },
            ),
            HostRet::Unit
        );
        // A foreign-prefix write is rejected...
        assert!(matches!(
            handle_host_call(
                &mut beta,
                HostCall::WorldKvSet {
                    key: "alpha:x".into(),
                    value: vec![9],
                },
            ),
            HostRet::Error(_)
        ));
        // ...and so are bare / degenerate keys.
        for bad in ["x", "alpha:", "petramond:", "alphax:y", "beta"] {
            assert!(
                matches!(
                    handle_host_call(
                        &mut beta,
                        HostCall::WorldKvSet {
                            key: bad.into(),
                            value: vec![1],
                        },
                    ),
                    HostRet::Error(_)
                ),
                "write with key '{bad}' must be rejected"
            );
        }
        // The rejected write changed nothing; a cross-namespace READ works.
        assert_eq!(
            handle_host_call(
                &mut beta,
                HostCall::WorldKvGet {
                    key: "alpha:x".into(),
                },
            ),
            HostRet::Bytes(Some(vec![7]))
        );
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::WorldKvGet {
                    key: "petramond:time".into(),
                },
            ),
            HostRet::Bytes(Some(vec![1]))
        );
        // Deletes are writes: foreign rejected, own applies.
        assert!(matches!(
            handle_host_call(
                &mut beta,
                HostCall::WorldKvDelete {
                    key: "alpha:x".into(),
                },
            ),
            HostRet::Error(_)
        ));
        assert_eq!(
            handle_host_call(
                &mut alpha,
                HostCall::WorldKvDelete {
                    key: "alpha:x".into(),
                },
            ),
            HostRet::Bool(true)
        );
        // The value size cap holds (same guard on every KV write surface).
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::WorldKvSet {
                    key: "alpha:big".into(),
                    value: vec![0; KV_MAX_VALUE_BYTES + 1],
                },
            ),
            HostRet::Error(_)
        ));
    });
    // Outside any dispatch scope, sim-touching KV calls are rejected.
    assert!(matches!(
        handle_host_call(
            &mut alpha,
            HostCall::WorldKvGet {
                key: "alpha:x".into(),
            },
        ),
        HostRet::Error(_)
    ));
}

/// The per-cell AGGREGATE cap: one more DISTINCT key on a cell already
/// holding `CELL_KV_MAX_KEYS` errors, while overwriting an existing key
/// at the cap passes (the cap bounds the map, not writes) and removing a
/// key frees a slot. The cap is what keeps every `BlockDelta` — which
/// ships the cell's whole KV map — a bounded wire payload.
#[test]
fn section_kv_caps_distinct_keys_per_cell() {
    use crate::modding::host::guards::CELL_KV_MAX_KEYS;
    let mut alpha = ModStoreData::new("alpha", 1);
    with_ctx(|| {
        let pos = [2, 65, 2];
        let set = |m: &mut ModStoreData, key: String| {
            handle_host_call(
                m,
                HostCall::SectionKvSet {
                    pos,
                    key,
                    value: vec![1],
                },
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
                HostRet::Error(_)
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
                HostCall::SectionKvDelete {
                    pos,
                    key: "alpha:k1".into(),
                },
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
