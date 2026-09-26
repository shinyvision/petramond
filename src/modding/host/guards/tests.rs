use mod_api::calls;
use mod_api::{BlockId, HostCall, HostRet};

use crate::events::tick::TickEvents;
use crate::events::{PostQueue, RosterRefs, SimCtx};
use crate::modding::host::{handle_host_call, ModStoreData};
use crate::modding::scope;
use crate::world::World;
use petramond_world::block::Block;

use super::read_only_permits;

/// A world with one stone-floored loaded chunk, a world KV entry and a cell
/// KV entry at (1, 64, 1) — something for every delete to target.
fn fixture_world() -> World {
    let mut world = World::new(1, 1);
    let mut c = petramond_world::chunk::Chunk::new(0, 0);
    for z in 0..petramond_world::chunk::CHUNK_SZ {
        for x in 0..petramond_world::chunk::CHUNK_SX {
            c.set_block(x, 64, z, Block::Stone);
        }
    }
    world.insert_chunk_for_test(petramond_world::chunk::ChunkPos::new(0, 0), c);
    world.world_kv_set("fixture:k".into(), vec![1]);
    assert!(world.cell_kv_set(1, 64, 1, "fixture:c".into(), vec![2]));
    world
}

/// The read-only promise is the HOST's, not each handler's: during a
/// read-only dispatch every write — including the deletes that once went
/// through the read wrapper — is refused, and the world is untouched, while
/// the listed reads still answer.
#[test]
fn a_read_only_dispatch_refuses_every_write_and_answers_reads() {
    let mut world = fixture_world();
    let mut store = ModStoreData::new("fixture", 1);
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
    scope::enter_read_only(&mut ctx, || {
        let writes = [
            HostCall::from(calls::WorldKvDelete {
                key: "fixture:k".into(),
            }),
            HostCall::from(calls::SectionKvDelete {
                pos: [1, 64, 1],
                key: "fixture:c".into(),
            }),
            HostCall::from(calls::MobTagDelete {
                mob_id: 0,
                key: "fixture:t".into(),
            }),
            HostCall::from(calls::SetBlock {
                pos: [1, 64, 1],
                block: BlockId(Block::Air.id()),
            }),
            HostCall::from(calls::SectionKvSet {
                pos: [1, 64, 1],
                key: "fixture:c".into(),
                value: vec![9],
            }),
            HostCall::from(calls::WorldKvSet {
                key: "fixture:k".into(),
                value: vec![9],
            }),
            HostCall::from(calls::MemoPut {
                key: b"k".to_vec(),
                value: b"v".to_vec(),
            }),
        ];
        for call in writes {
            let what = format!("{call:?}");
            assert!(
                matches!(handle_host_call(&mut store, call), HostRet::Err(_)),
                "{what} must be refused in a read-only dispatch"
            );
        }
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::WorldKvGet {
                    key: "fixture:k".into()
                })
            ),
            HostRet::Bytes(Some(vec![1]))
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::SectionKvGet {
                    pos: [1, 64, 1],
                    key: "fixture:c".into()
                })
            ),
            HostRet::Bytes(Some(vec![2]))
        );
        assert_eq!(
            handle_host_call(&mut store, HostCall::from(calls::GetBlock { pos: [1, 64, 1] })),
            HostRet::Block(Some(BlockId(Block::Stone.id())))
        );
        assert_eq!(
            handle_host_call(&mut store, HostCall::from(calls::CurrentTick)),
            HostRet::U64(0)
        );
    });
    assert_eq!(world.world_kv_get("fixture:k"), Some(&[1u8][..]));
    assert_eq!(world.cell_kv_get(1, 64, 1, "fixture:c"), Some(&[2u8][..]));
    assert_eq!(world.block_if_stream_final(1, 64, 1), Some(Block::Stone));
}

/// Outside a read-only dispatch the same deletes work — the refusal above
/// is the scope's, not a broken handler.
#[test]
fn an_ordinary_dispatch_still_deletes() {
    let mut world = fixture_world();
    let mut store = ModStoreData::new("fixture", 1);
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
    scope::enter(&mut ctx, || {
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::WorldKvDelete {
                    key: "fixture:k".into()
                })
            ),
            HostRet::Bool(true)
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::from(calls::SectionKvDelete {
                    pos: [1, 64, 1],
                    key: "fixture:c".into()
                })
            ),
            HostRet::Bool(true)
        );
    });
    assert_eq!(world.world_kv_get("fixture:k"), None);
    assert_eq!(world.cell_kv_get(1, 64, 1, "fixture:c"), None);
}

#[test]
fn the_read_only_allow_list_admits_reads_only() {
    assert!(read_only_permits(&HostCall::from(calls::GetBlock { pos: [0, 0, 0] })));
    assert!(read_only_permits(&HostCall::from(calls::WorldKvGet { key: "a:b".into() })));
    assert!(!read_only_permits(&HostCall::from(calls::WorldKvDelete { key: "a:b".into() })));
    assert!(!read_only_permits(&HostCall::from(calls::SectionKvDelete {
        pos: [0, 0, 0],
        key: "a:b".into()
    })));
    assert!(!read_only_permits(&HostCall::from(calls::MobTagDelete {
        mob_id: 1,
        key: "a:b".into()
    })));
    assert!(!read_only_permits(&HostCall::from(calls::SetBlocks { blocks: Vec::new() })));
}
