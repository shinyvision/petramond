use mod_api::{HostCall, HostRet};

use crate::events::tick::TickEvents;
use crate::events::{PostQueue, SimCtx};
use crate::modding::host::{handle_host_call, ModStoreData};
use crate::modding::scope;
use crate::player::Player;
use crate::world::World;
use petramond_math::world_pos::WorldPos;
use petramond_world::chunk::ChunkPos;

/// Container host calls canonicalize any footprint cell of a multi-cell
/// model block to the group ANCHOR: a write through a non-anchor cell
/// must land in the one anchored container (the same slots the GUI and
/// break-scatter use), never mint a second store at that cell.
#[test]
fn container_calls_canonicalize_to_the_group_anchor() {
    let mut world = World::new(1, 4);
    world.clear_world();
    world.insert_chunk_for_test(
        ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let origin = petramond_math::math::IVec3::new(5, 64, 5);
    assert!(world.place_model_block(origin, petramond_world::block::Block::FurnitureWorkbench));
    let (_, anchor, cells) = world.model_group(origin).expect("a placed model group");
    let far = *cells
        .iter()
        .find(|c| **c != anchor)
        .expect("a non-anchor cell");

    // The workbench is engine-owned and ContainerSet is guarded to the
    // caller's own namespace, so the test store impersonates the engine
    // namespace — this keeps the test off the heavy WASM fixture.
    let mut store = ModStoreData::new(petramond_world::registry::ENGINE_NAMESPACE, 1);
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
    scope::enter(&mut ctx, || {
        let set = handle_host_call(
            &mut store,
            HostCall::ContainerSet {
                pos: far.to_array(),
                slots: vec![(
                    0,
                    Some(mod_api::ItemStackData {
                        item: "petramond:coal".into(),
                        count: 3,
                        data: Vec::new(),
                    }),
                )],
            },
        );
        assert_eq!(set, HostRet::Bool(true));
        // Reading through a different cell (the anchor) sees the write.
        let got = handle_host_call(
            &mut store,
            HostCall::ContainerGet {
                pos: anchor.to_array(),
            },
        );
        let HostRet::ContainerSlots(Some(slots)) = got else {
            panic!("expected slots from the anchor, got {got:?}");
        };
        assert_eq!(slots[0].as_ref().map(|s| s.count), Some(3));
    });
    // One container, keyed at the anchor — nothing stranded at the cell.
    assert!(world.container_at(anchor).is_some());
    assert!(world.container_at(far).is_none());
}

#[test]
fn transfers_respect_target_admission_and_preserve_items_on_failure() {
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;
    use petramond_world::item::{ItemStack, ItemType};
    let mut world = World::new(1, 4);
    world.clear_world();
    world.insert_chunk_for_test(
        ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let machine = IVec3::new(1, 64, 1);
    let chest = IVec3::new(3, 64, 1);
    world.set_block_world(machine.x, machine.y, machine.z, Block::Furnace);
    world.set_block_world(chest.x, chest.y, chest.z, Block::Chest);
    world.take_container(chest);
    let mut store = ModStoreData::new("transfer_test", 1);
    let mut player = Player::new(WorldPos::new(0.0, 80.0, 0.0));
    let mut feed = TickEvents::default();
    let mut queue = PostQueue::default();
    let mut gui = petramond_world::gui_state::empty_gui_state();
    let stack = |item: &str, count| mod_api::ItemStackData {
        item: item.into(),
        count,
        data: vec![("transfer_test:mark".into(), vec![9])],
    };
    let mut ctx = SimCtx {
        world: &mut world,
        player: &mut player,
        gui_state: &mut gui,
        feed: &mut feed,
        queue: &mut queue,
    };
    scope::enter(&mut ctx, || {
        let coal = stack("petramond:coal", 3);
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerInsert {
                    pos: machine.to_array(),
                    stack: coal.clone()
                }
            ),
            HostRet::ItemStack(None)
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerTake {
                    pos: machine.to_array(),
                    slot: petramond_world::furnace::SLOT_FUEL as u32,
                    count: 2
                }
            ),
            HostRet::ItemStack(Some(stack("petramond:coal", 2)))
        );
        let stone = stack("petramond:stone", 4);
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerInsert {
                    pos: machine.to_array(),
                    stack: stone.clone()
                }
            ),
            HostRet::ItemStack(Some(stone.clone()))
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerInsert {
                    pos: chest.to_array(),
                    stack: stone
                }
            ),
            HostRet::ItemStack(None)
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerTake {
                    pos: chest.to_array(),
                    slot: 0,
                    count: 9
                }
            ),
            HostRet::ItemStack(Some(stack("petramond:stone", 4)))
        );
        let coal = stack("petramond:coal", 2);
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerInsert {
                    pos: [6, 64, 1],
                    stack: coal.clone()
                }
            ),
            HostRet::ItemStack(Some(coal))
        );
    });
    let c = world.container_at_mut(chest).unwrap();
    c.slots.fill(Some(ItemStack::new(
        ItemType::Stone,
        ItemType::Stone.max_stack_size(),
    )));
    let mut ctx = SimCtx {
        world: &mut world,
        player: &mut player,
        gui_state: &mut gui,
        feed: &mut feed,
        queue: &mut queue,
    };
    scope::enter(&mut ctx, || {
        let incoming = stack("petramond:coal", 3);
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerInsert {
                    pos: chest.to_array(),
                    stack: incoming.clone()
                }
            ),
            HostRet::ItemStack(Some(incoming))
        );
    });
}
