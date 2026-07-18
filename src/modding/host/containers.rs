//! Container calls: engine-backed mod container slots plus the machine
//! recipe reads machine mods compose with. (Item registry reads live in the
//! `registry` domain.)

use mod_api::{HostCall, HostRet};

use super::guards::{
    batch_guard, item_by_name, item_stack_data, key_owned_by_namespace, sim_query,
    stream_final_cell,
};

/// Mod container slots + the machine recipe read that makes furnace-like
/// mod logic possible without duplicating engine data.
pub(super) fn handle_container_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::ContainerGet { pos } => sim_query(|ctx| {
            // Multi-cell blocks keep ONE container at the group anchor;
            // canonicalize so any footprint cell reads the same slots the GUI
            // and break-scatter use.
            let p = ctx.world.container_anchor(pos.into());
            HostRet::ContainerSlots(ctx.world.container_at(p).map(|c| {
                c.slots
                    .iter()
                    .map(|slot| slot.map(item_stack_data))
                    .collect()
            }))
        }),
        HostCall::ContainerGetMany { positions } => {
            if let Some(err) = batch_guard("ContainerGetMany position", positions.len()) {
                return err;
            }
            sim_query(|ctx| {
                HostRet::Containers(
                    positions
                        .iter()
                        .map(|&pos| {
                            let p = ctx.world.container_anchor(pos.into());
                            ctx.world.container_at(p).map(|c| {
                                c.slots
                                    .iter()
                                    .map(|slot| slot.map(item_stack_data))
                                    .collect()
                            })
                        })
                        .collect(),
                )
            })
        }
        HostCall::ContainerSet { pos, slots } => {
            if let Some(err) = batch_guard("ContainerSet slot entry", slots.len()) {
                return err;
            }
            // Resolve+validate every entry BEFORE any write, so a bad entry
            // can't leave a half-applied batch.
            let mut writes: Vec<(usize, Option<crate::item::ItemStack>)> = Vec::new();
            for (i, slot) in &slots {
                let i = *i as usize;
                if i >= crate::container::MAX_CONTAINER_SLOTS {
                    return HostRet::Error(format!(
                        "ContainerSet: slot {i} is past the cap ({})",
                        crate::container::MAX_CONTAINER_SLOTS
                    ));
                }
                let stack = match slot {
                    None => None,
                    Some(data) => {
                        // A typo'd registry name is not a protocol break: warn
                        // and refuse the batch (the GiveItem/EffectApply
                        // policy), don't trap the whole mod.
                        let Some(item) = item_by_name(&data.item) else {
                            log::warn!(
                                "[mod {mod_id}] ContainerSet: unknown item '{}' — \
                                 batch not applied",
                                data.item
                            );
                            return HostRet::Bool(false);
                        };
                        (data.count > 0).then(|| crate::item::ItemStack::new(item, data.count))
                    }
                };
                writes.push((i, stack));
            }
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                // Same anchor rule as ContainerGet: writing through a
                // non-anchor footprint cell must not mint a second container
                // the GUI and break-scatter would never see.
                let p = ctx.world.container_anchor(pos.into());
                // A mod owns only its own blocks' containers: the block at
                // `pos` must be registered to the caller's namespace.
                let block = match stream_final_cell(ctx, p) {
                    Ok(b) => b,
                    Err(miss) => return miss,
                };
                let block_name = crate::registry::names()
                    .blocks
                    .name(block.id())
                    .unwrap_or("?");
                if !key_owned_by_namespace(&mod_id, block_name) {
                    return HostRet::Error(format!(
                        "ContainerSet: block '{block_name}' at {pos:?} is not owned by mod \
                         '{mod_id}' (writes are namespace-guarded; reads may cross)"
                    ));
                }
                let len = writes.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
                if !ctx.world.ensure_container(p, len) {
                    return HostRet::Bool(false);
                }
                if let Some(container) = ctx.world.container_at_mut(p) {
                    for (i, stack) in writes {
                        container.slots[i] = stack;
                    }
                }
                ctx.world.mark_chunk_modified(p);
                HostRet::Bool(true)
            })
        }
        HostCall::RecipeResult { class, item } => {
            let Some(recipes) = crate::modding::active_recipes() else {
                log::warn!("[mod {mod_id}] RecipeResult: no recipe catalog installed");
                return HostRet::ItemStack(None);
            };
            let Some(item) = item_by_name(&item) else {
                return HostRet::ItemStack(None);
            };
            HostRet::ItemStack(recipes.process(&class, item).map(item_stack_data))
        }
        other => HostRet::Error(format!(
            "non-container call {other:?} mis-routed to handle_container_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::chunk::ChunkPos;
    use crate::events::{PostQueue, SimCtx};
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    /// Container host calls canonicalize any footprint cell of a multi-cell
    /// model block to the group ANCHOR: a write through a non-anchor cell
    /// must land in the one anchored container (the same slots the GUI and
    /// break-scatter use), never mint a second store at that cell.
    #[test]
    fn container_calls_canonicalize_to_the_group_anchor() {
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_chunk_for_test(ChunkPos::new(0, 0), crate::chunk::Chunk::new(0, 0));
        let origin = crate::mathh::IVec3::new(5, 64, 5);
        assert!(world.place_model_block(origin, crate::block::Block::FurnitureWorkbench));
        let (_, anchor, cells) = world.model_group(origin).expect("a placed model group");
        let far = *cells
            .iter()
            .find(|c| **c != anchor)
            .expect("a non-anchor cell");

        // The workbench is engine-owned and ContainerSet is guarded to the
        // caller's own namespace, so the test store impersonates the engine
        // namespace — this keeps the test off the heavy WASM fixture.
        let mut store = ModStoreData::new(crate::registry::ENGINE_NAMESPACE, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
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
}
