//! Container calls: engine-backed mod container slots plus the item and
//! recipe registry reads machine mods compose with.

use mod_api::{HostCall, HostRet};

use crate::modding::convert::to_ivec;

use super::guards::{item_by_key, item_stack_data, key_owned_by_namespace, sim_query};

/// Mod container slots + the item/recipe registry reads that make furnace-like
/// mod logic possible without duplicating engine data.
pub(super) fn handle_container_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::ContainerGet { pos } => sim_query(|ctx| {
            // Multi-cell blocks keep ONE container at the group anchor;
            // canonicalize so any footprint cell reads the same slots the GUI
            // and break-scatter use.
            let p = ctx.world.container_anchor(to_ivec(pos));
            HostRet::ContainerSlots(ctx.world.container_at(p).map(|c| {
                c.slots
                    .iter()
                    .map(|slot| slot.map(item_stack_data))
                    .collect()
            }))
        }),
        HostCall::ContainerGetMany { positions } => sim_query(|ctx| {
            HostRet::Containers(
                positions
                    .iter()
                    .map(|&pos| {
                        let p = ctx.world.container_anchor(to_ivec(pos));
                        ctx.world.container_at(p).map(|c| {
                            c.slots
                                .iter()
                                .map(|slot| slot.map(item_stack_data))
                                .collect()
                        })
                    })
                    .collect(),
            )
        }),
        HostCall::ContainerSet { pos, slots } => {
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
                        // A typo'd registry key is not a protocol break: warn
                        // and refuse the batch (the GiveItem/EffectApply
                        // policy), don't trap the whole mod.
                        let Some(item) = item_by_key(&data.key) else {
                            log::warn!(
                                "[mod {mod_id}] ContainerSet: unknown item '{}' — \
                                 batch not applied",
                                data.key
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
                let p = ctx.world.container_anchor(to_ivec(pos));
                // A mod owns only its own blocks' containers: the block at
                // `pos` must be registered to the caller's namespace.
                // Stream-final read: a half-streamed cell shows the generated
                // base (a foreign block) — that must be "not stored", not a
                // namespace violation.
                let Some(block) = ctx.world.block_if_stream_final(p.x, p.y, p.z) else {
                    return HostRet::Bool(false);
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
        HostCall::ItemInfo { key } => {
            HostRet::ItemInfo(item_by_key(&key).map(|item| mod_api::ItemInfoData {
                max_stack: item.max_stack_size(),
                fuel_burn_ticks: item.fuel_burn_ticks() as u32,
                tags: item.tags().iter().map(|t| t.name().to_owned()).collect(),
            }))
        }
        // Registry-only like ResolveBlock: legal on any instance, any time —
        // how a mod matches the numeric ids in event payloads (item_use_pre)
        // against its own names.
        HostCall::ResolveItem { key } => {
            HostRet::Item(crate::registry::names().items.id(&key).map(mod_api::ItemId))
        }
        HostCall::RecipeResult { class, key } => {
            let Some(recipes) = crate::modding::active_recipes() else {
                log::warn!("[mod {mod_id}] RecipeResult: no recipe catalog installed");
                return HostRet::ItemStack(None);
            };
            let Some(item) = item_by_key(&key) else {
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

    /// ResolveItem is registry-only: it answers OUTSIDE any published SimCtx
    /// (the ResolveBlock contract), returns the session id for a known name,
    /// and `None` — never an error — for an unknown one.
    #[test]
    fn resolve_item_answers_without_a_sim_scope() {
        let mut store = ModStoreData::new("somemod", 1);
        let got = handle_host_call(
            &mut store,
            HostCall::ResolveItem {
                key: "petramond:stick".into(),
            },
        );
        let HostRet::Item(Some(id)) = got else {
            panic!("expected a resolved id for petramond:stick, got {got:?}");
        };
        assert_eq!(id.0, crate::item::ItemType::Stick.id());
        let unknown = handle_host_call(
            &mut store,
            HostCall::ResolveItem {
                key: "somemod:not_a_thing".into(),
            },
        );
        assert_eq!(unknown, HostRet::Item(None));
    }

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
                    pos: [far.x, far.y, far.z],
                    slots: vec![(
                        0,
                        Some(mod_api::ItemStackData {
                            key: "petramond:coal".into(),
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
                    pos: [anchor.x, anchor.y, anchor.z],
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
