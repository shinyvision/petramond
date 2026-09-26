use super::*;
use crate::world::test_world::TestWorld;

#[test]
fn namespaced_keys_intern_to_shared_singletons_that_enqueue_hooks() {
    let a = crate::block::behavior::by_name("testmod:zap")
        .expect("namespaced keys resolve");
    let b = crate::block::behavior::by_name("testmod:zap").expect("stable");
    assert_eq!(a.key(), "testmod:zap", "key() inverts by_name()");
    assert!(
        std::ptr::eq(a as *const _ as *const u8, b as *const _ as *const u8),
        "one singleton per key"
    );
    assert!(a.has_random_tick());
    assert!(
        crate::block::behavior::by_name("bogus").is_none(),
        "bare unknowns still error"
    );

    let mut world = TestWorld::new(0);
    let pos = IVec3::new(1, 65, 1);
    a.random_tick(&mut world, pos);
    a.neighbor_update(&mut world, pos);
    let hooks = world.data.take_block_hooks();
    assert_eq!(hooks.len(), 2);
    assert_eq!(hooks[0].kind, BlockHookKind::RandomTick);
    assert_eq!(hooks[1].kind, BlockHookKind::NeighborUpdate);
    assert_eq!(hooks[0].key, "testmod:zap");
    assert_eq!(hooks[0].pos, pos);
    assert!(world.data.take_block_hooks().is_empty(), "take drains");
}
