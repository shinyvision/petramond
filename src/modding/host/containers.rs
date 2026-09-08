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
        HostCall::ContainerInsert { pos, stack } => {
            let Some(item) = item_by_name(&stack.item) else {
                return HostRet::ItemStack(Some(stack));
            };
            if stack.count > item.max_stack_size() {
                return HostRet::ItemStack(Some(stack));
            }
            let variant = match super::guards::intern_abi_data("ContainerInsert", &stack.data) {
                Ok(v) => v,
                Err(e) => return e,
            };
            sim_query(|ctx| {
                let p = ctx.world.container_anchor(pos.into());
                let mut remainder = (stack.count > 0).then(|| {
                    petramond_world::item::ItemStack::with_variant(item, stack.count, variant)
                });
                if let Ok(block) = stream_final_cell(ctx, p) {
                    if let petramond_world::block::BlockInteraction::OpenGui(kind) =
                        block.interaction()
                    {
                        let specs = crate::menu::slot_specs_for_kind(kind);
                        if !specs.is_empty() && ctx.world.ensure_container(p, specs.len()) {
                            if let Some(c) = ctx.world.container_at_mut(p) {
                                petramond_world::container::route_into(
                                    &mut remainder,
                                    &mut c.slots,
                                    &specs,
                                    None,
                                );
                            }
                            ctx.world.mark_chunk_modified(p);
                        }
                    }
                }
                HostRet::ItemStack(remainder.map(item_stack_data))
            })
        }
        HostCall::ContainerTake { pos, slot, count } => sim_query(|ctx| {
            let p = ctx.world.container_anchor(pos.into());
            let taken = if stream_final_cell(ctx, p).is_ok() && count > 0 {
                ctx.world
                    .container_at_mut(p)
                    .and_then(|c| c.slots.get_mut(slot as usize))
                    .and_then(|cell| {
                        let stack = (*cell)?;
                        let n = stack.count.min(count);
                        *cell = (stack.count > n).then(|| stack.restack(stack.count - n));
                        Some(stack.restack(n))
                    })
            } else {
                None
            };
            if taken.is_some() {
                ctx.world.mark_chunk_modified(p);
            }
            HostRet::ItemStack(taken.map(item_stack_data))
        }),
        HostCall::ContainerSet { pos, slots } => {
            if let Some(err) = batch_guard("ContainerSet slot entry", slots.len()) {
                return err;
            }
            // Resolve+validate every entry BEFORE any write, so a bad entry
            // can't leave a half-applied batch.
            let mut writes: Vec<(usize, Option<petramond_world::item::ItemStack>)> = Vec::new();
            for (i, slot) in &slots {
                let i = *i as usize;
                if i >= petramond_world::container::MAX_CONTAINER_SLOTS {
                    return HostRet::Error(format!(
                        "ContainerSet: slot {i} is past the cap ({})",
                        petramond_world::container::MAX_CONTAINER_SLOTS
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
                        {
                            let variant =
                                match super::guards::intern_abi_data("ContainerSet", &data.data) {
                                    Ok(v) => v,
                                    Err(e) => return e,
                                };
                            (data.count > 0).then(|| {
                                petramond_world::item::ItemStack::with_variant(
                                    item, data.count, variant,
                                )
                            })
                        }
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
                let block_name = petramond_world::registry::names()
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
mod tests;
