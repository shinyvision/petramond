//! Container calls: engine-backed mod container slots plus the machine
//! recipe reads machine mods compose with. (Item registry reads live in the
//! `registry` domain.)

use mod_api::{HostCall, HostRet};

use crate::events::SimCtx;

use super::guards::{
    batch_guard, item_by_name, item_stack_data, key_owned_by_namespace, sim_mutating_query,
    sim_query,
};

mod access;

/// Mod container slots + the machine recipe read that makes furnace-like
/// mod logic possible without duplicating engine data.
pub(super) fn handle_container_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::ContainerGet { at } => {
            sim_query(|ctx| HostRet::ContainerSlots(read_slots(ctx, at)))
        }
        HostCall::ContainerGetMany { addresses } => {
            if let Some(err) = batch_guard("ContainerGetMany address", addresses.len()) {
                return err;
            }
            sim_query(|ctx| {
                HostRet::Containers(addresses.iter().map(|&at| read_slots(ctx, at)).collect())
            })
        }
        HostCall::ContainerInsert { at, stack } => {
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
            sim_mutating_query(|ctx| {
                let mut remainder = (stack.count > 0).then(|| {
                    petramond_world::item::ItemStack::with_variant(item, stack.count, variant)
                });
                insert(ctx, at, &mut remainder);
                HostRet::ItemStack(remainder.map(item_stack_data))
            })
        }
        HostCall::ContainerHold { at, actor, open } => sim_mutating_query(|ctx| {
            let mod_api::EntityRef::Mob(mob_id) = actor else {
                return HostRet::Bool(false);
            };
            // A holder is a viewer: whatever really stores slots can be
            // held, by the same resolution every container call reads through.
            let Some(access::Target::Block(pos)) = access::resolve_read(ctx, at) else {
                return HostRet::Bool(false);
            };
            let stores = access::slots(ctx, access::Target::Block(pos)).is_some();
            if !stores || ctx.world.mobs().index_of_id(mob_id).is_none() {
                return HostRet::Bool(false);
            }
            ctx.queue
                .push_action(crate::events::DeferredAction::ContainerHold { mob_id, pos, open });
            HostRet::Bool(true)
        }),
        HostCall::ContainerTake { at, slot, count } => sim_mutating_query(|ctx| {
            HostRet::ItemStack(take(ctx, at, slot, count).map(item_stack_data))
        }),
        HostCall::ContainerTransfer {
            from,
            slot,
            to,
            count,
        } => sim_mutating_query(|ctx| {
            // Both ends must be writable before anything leaves the source,
            // so a refused destination costs nothing.
            if access::resolve_write(ctx, to).is_none() {
                return HostRet::ItemStack(None);
            }
            let Some(taken) = take(ctx, from, slot, count) else {
                return HostRet::ItemStack(None);
            };
            let mut remainder = Some(taken);
            insert(ctx, to, &mut remainder);
            let moved = match remainder {
                None => Some(taken),
                Some(left) if left.count < taken.count => {
                    Some(taken.restack(taken.count - left.count))
                }
                Some(_) => None,
            };
            if let Some(left) = remainder {
                // The source slot gave up `taken` a moment ago, so it has room
                // for exactly what comes back.
                let source = access::resolve_write(ctx, from).expect("source resolved by take");
                let cell = access::slots_mut(ctx, source)
                    .and_then(|c| c.slots.get_mut(slot as usize))
                    .expect("source slot existed for take");
                match cell {
                    Some(stack) => stack.count += left.count,
                    None => *cell = Some(left),
                }
                access::touched(ctx, source);
            }
            HostRet::ItemStack(moved.map(item_stack_data))
        }),
        HostCall::ContainerSet { at, slots } => {
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
            sim_mutating_query(move |ctx| {
                let Some(target) = access::resolve_write(ctx, at) else {
                    return HostRet::Bool(false);
                };
                // A mod owns only its own storage: the block at the anchor, or
                // the mob's species, must be registered to the caller.
                let Some(owner) = access::owner_name(ctx, target) else {
                    return HostRet::Bool(false);
                };
                // Whose storage stands there is the world's to say and changes
                // under a mod: someone else's answers `false`, never an error.
                if !key_owned_by_namespace(&mod_id, owner) {
                    log::debug!("ContainerSet: '{owner}' at {at:?} is not mod '{mod_id}''s");
                    return HostRet::Bool(false);
                }
                let len = writes.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
                match target {
                    access::Target::Block(p) => {
                        if !ctx.world.ensure_container(p, len) {
                            return HostRet::Bool(false);
                        }
                    }
                    // A mob's capacity is its row's: a write never grows it.
                    access::Target::Mob(_) => {
                        if access::slots(ctx, target).is_none_or(|c| c.slots.len() < len) {
                            return HostRet::Bool(false);
                        }
                    }
                }
                if let Some(container) = access::slots_mut(ctx, target) {
                    for (i, stack) in writes {
                        container.slots[i] = stack;
                    }
                }
                access::touched(ctx, target);
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

fn read_slots(
    ctx: &SimCtx<'_>,
    at: mod_api::ContainerAddress,
) -> Option<Vec<Option<mod_api::ItemStackData>>> {
    let target = access::resolve_read(ctx, at)?;
    access::slots(ctx, target).map(|c| c.slots.iter().map(|s| s.map(item_stack_data)).collect())
}

/// Route `stack` into the container `at` through its admission rules,
/// leaving whatever it refuses in `stack`.
fn insert(
    ctx: &mut SimCtx<'_>,
    at: mod_api::ContainerAddress,
    stack: &mut Option<petramond_world::item::ItemStack>,
) {
    let Some(target) = access::resolve_write(ctx, at) else {
        return;
    };
    let Some(specs) = access::insert_specs(ctx, target) else {
        return;
    };
    if let Some(container) = access::slots_mut(ctx, target) {
        petramond_world::container::route_into(stack, &mut container.slots, &specs, None);
    }
    access::touched(ctx, target);
}

/// Take at most `count` from one slot of the container `at`.
fn take(
    ctx: &mut SimCtx<'_>,
    at: mod_api::ContainerAddress,
    slot: u32,
    count: u8,
) -> Option<petramond_world::item::ItemStack> {
    let target = access::resolve_write(ctx, at).filter(|_| count > 0)?;
    let taken = access::slots_mut(ctx, target)
        .and_then(|c| c.slots.get_mut(slot as usize))
        .and_then(|cell| {
            let stack = (*cell)?;
            let n = stack.count.min(count);
            *cell = (stack.count > n).then(|| stack.restack(stack.count - n));
            Some(stack.restack(n))
        })?;
    access::touched(ctx, target);
    Some(taken)
}

#[cfg(test)]
mod tests;
