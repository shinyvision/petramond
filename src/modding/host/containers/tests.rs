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
                at: far.to_array().into(),
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
                at: anchor.to_array().into(),
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
                    at: machine.to_array().into(),
                    stack: coal.clone()
                }
            ),
            HostRet::ItemStack(None)
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerTake {
                    at: machine.to_array().into(),
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
                    at: machine.to_array().into(),
                    stack: stone.clone()
                }
            ),
            HostRet::ItemStack(Some(stone.clone()))
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerInsert {
                    at: chest.to_array().into(),
                    stack: stone
                }
            ),
            HostRet::ItemStack(None)
        );
        assert_eq!(
            handle_host_call(
                &mut store,
                HostCall::ContainerTake {
                    at: chest.to_array().into(),
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
                    at: [6, 64, 1].into(),
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
                    at: chest.to_array().into(),
                    stack: incoming.clone()
                }
            ),
            HostRet::ItemStack(Some(incoming))
        );
    });
}

/// A transfer is one move: what the destination's admission refuses stays in
/// the source slot, so no item is ever in both containers or neither.
#[test]
fn a_transfer_moves_only_what_the_destination_admits() {
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;
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
    let mut store = ModStoreData::new("transfer_test", 1);
    let mut player = Player::new(WorldPos::new(0.0, 80.0, 0.0));
    let mut feed = TickEvents::default();
    let mut queue = PostQueue::default();
    let mut gui = petramond_world::gui_state::empty_gui_state();
    let coal = |count| mod_api::ItemStackData {
        item: "petramond:coal".into(),
        count,
        data: Vec::new(),
    };
    let mut ctx = SimCtx {
        world: &mut world,
        player: &mut player,
        gui_state: &mut gui,
        feed: &mut feed,
        queue: &mut queue,
    };
    scope::enter(&mut ctx, || {
        let mut call = |c| handle_host_call(&mut store, c);
        // The furnace's only coal cell is its fuel slot, nearly full.
        assert_eq!(
            call(HostCall::ContainerInsert {
                at: machine.to_array().into(),
                stack: coal(60),
            }),
            HostRet::ItemStack(None)
        );
        assert_eq!(
            call(HostCall::ContainerInsert {
                at: chest.to_array().into(),
                stack: coal(64),
            }),
            HostRet::ItemStack(None)
        );
        assert_eq!(
            call(HostCall::ContainerTransfer {
                from: chest.to_array().into(),
                slot: 0,
                to: machine.to_array().into(),
                count: 64,
            }),
            HostRet::ItemStack(Some(coal(4))),
            "only the fuel slot's room moves"
        );
        let HostRet::ContainerSlots(Some(slots)) = call(HostCall::ContainerGet {
            at: chest.to_array().into(),
        }) else {
            panic!("the chest has slots");
        };
        assert_eq!(slots[0], Some(coal(60)), "the refused part stayed put");
        assert_eq!(
            call(HostCall::ContainerTransfer {
                from: chest.to_array().into(),
                slot: 0,
                to: [9, 64, 9].into(),
                count: 1,
            }),
            HostRet::ItemStack(None),
            "a destination with no slots takes nothing"
        );
    });
}

/// A mob's carried slots are an ordinary container addressed by its stable
/// id, and whatever it still carries when it leaves the world scatters.
#[test]
fn a_mobs_carried_slots_are_a_container_and_spill_when_it_leaves() {
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;
    use petramond_world::container::Container;
    use petramond_world::item::{ItemStack, ItemType};
    let mut world = World::new(1, 4);
    world.clear_world();
    world.insert_chunk_for_test(
        ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let chest = IVec3::new(3, 64, 1);
    world.set_block_world(chest.x, chest.y, chest.z, Block::Chest);
    world.mobs_mut().restore([crate::mob::SavedMob {
        kind: crate::mob::Mob::Owl,
        pos: WorldPos::new(8.5, 64.0, 8.5),
        yaw: 0.0,
        tags: Default::default(),
        container: Container {
            slots: vec![
                Some(ItemStack::new(ItemType::Coal, 5)),
                Some(ItemStack::new(ItemType::Stone, 2)),
            ],
        },
    }]);
    let mob = world.mobs().instances()[0].id();
    let mut store = ModStoreData::new("transfer_test", 1);
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
        let mut call = |c| handle_host_call(&mut store, c);
        assert_eq!(
            call(HostCall::ContainerTransfer {
                from: mod_api::ContainerAddress::Mob(mob),
                slot: 0,
                to: chest.to_array().into(),
                count: 5,
            }),
            HostRet::ItemStack(Some(mod_api::ItemStackData {
                item: "petramond:coal".into(),
                count: 5,
                data: Vec::new(),
            }))
        );
        assert!(
            matches!(
                call(HostCall::ContainerSet {
                    at: mod_api::ContainerAddress::Mob(mob),
                    slots: vec![(0, None)],
                }),
                HostRet::Bool(false)
            ),
            "writes stay with the species' own pack"
        );
    });
    assert!(world.mobs_mut().remove(0));
    let spills = world.mobs_mut().take_spills();
    let spilled: Vec<_> = spills
        .iter()
        .flat_map(|s| s.stacks.iter().copied())
        .collect();
    assert_eq!(spilled, vec![ItemStack::new(ItemType::Stone, 2)]);
}
