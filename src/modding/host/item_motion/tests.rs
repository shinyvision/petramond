use super::*;
use petramond_math::world_pos::WorldPos;

#[test]
fn malformed_requests_are_rejected_before_entering_simulation() {
    for radius in [-1.0, f32::NAN, f32::INFINITY, 65.0] {
        let result = handle(HostCall::ItemEntitiesInRadius {
            pos: [0.0; 3],
            radius,
            limit: 1,
        });
        assert!(matches!(result, HostRet::Error(message) if message.contains("radius")));
    }
    let result = handle(HostCall::ItemEntitiesInRadius {
        pos: [0.0; 3],
        radius: 1.0,
        limit: mod_api::SIM_BATCH_MAX as u32 + 1,
    });
    assert!(matches!(result, HostRet::Error(message) if message.contains("limit")));
    let result = handle(HostCall::ItemImpulses {
        impulses: vec![(1, [0.0; 3]), (2, [f32::NAN; 3])],
    });
    assert!(matches!(result, HostRet::Error(message) if message.contains("delta")));
}

#[test]
fn read_only_dispatch_can_query_but_cannot_change_item_velocity() {
    use crate::entity::DroppedItem;
    use crate::events::{tick::TickEvents, PostQueue, SimCtx};
    use crate::modding::{
        host::{handle_host_call, ModStoreData},
        scope,
    };
    use crate::{player::Player, world::World};
    use petramond_math::math::Vec3;
    use petramond_world::{
        chunk::{Chunk, ChunkPos},
        item::{ItemStack, ItemType},
    };

    let mut world = World::new(1, 1);
    world.clear_world();
    world.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
    let pos = WorldPos::new(4.5, 64.5, 4.5);
    let id = world.spawn_item(DroppedItem::new(pos, ItemStack::new(ItemType::Dirt, 1), 1));
    let before = world.dropped_items().get(id).unwrap().vel;
    let mut player = Player::new(pos);
    let mut feed = TickEvents::default();
    let mut queue = PostQueue::default();
    let mut gui = petramond_world::gui_state::empty_gui_state();
    let mut store = ModStoreData::new("fixture", 1);
    let mut ctx = SimCtx {
        world: &mut world,
        player: &mut player,
        gui_state: &mut gui,
        feed: &mut feed,
        queue: &mut queue,
    };
    scope::enter_read_only(&mut ctx, || {
        let got = handle_host_call(
            &mut store,
            HostCall::ItemEntitiesInRadius {
                pos: pos.to_array(),
                radius: 1.0,
                limit: 1,
            },
        );
        assert!(
            matches!(got, HostRet::ItemEntities(items) if items.len() == 1 && items[0].id == id)
        );
        let denied = handle_host_call(
            &mut store,
            HostCall::ItemImpulses {
                impulses: vec![(id, [1.0, 0.0, 0.0])],
            },
        );
        assert!(matches!(denied, HostRet::Error(_)));
    });
    assert_eq!(ctx.world.dropped_items().get(id).unwrap().vel, before);
    scope::enter(&mut ctx, || {
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ItemImpulses {
                    impulses: vec![(id, [1.0, 0.0, 0.0])],
                }
            ),
            HostRet::Bools(vec![true])
        );
    });
    assert_eq!(world.dropped_items().get(id).unwrap().vel, before + Vec3::X);
}
