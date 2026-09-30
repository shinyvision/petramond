use mod_api::{HostRet, ItemMotionCall};

use super::entities::item_entity_data;
use super::guards::{batch_guard, finite3, sim_query, sim_read};

#[cfg(test)]
mod tests;

pub(super) fn handle(call: ItemMotionCall) -> HostRet {
    match call {
        ItemMotionCall::ItemEntitiesInRadius { pos, radius, limit } => {
            let pos = match super::guards::finite_pos(pos, "ItemEntitiesInRadius.pos") {
                Ok(pos) => pos,
                Err(error) => return error,
            };
            if !radius.is_finite() || !(0.0..=64.0).contains(&radius) {
                return HostRet::invalid("ItemEntitiesInRadius: radius must be in 0..=64".into());
            }
            if let Some(error) = batch_guard("ItemEntitiesInRadius.limit", limit as usize) {
                return error;
            }
            sim_read(|ctx| {
                HostRet::ItemEntities(
                    ctx.world
                        .nearest_item_entities(pos, radius, limit as usize)
                        .into_iter()
                        .map(item_entity_data)
                        .collect(),
                )
            })
        }
        ItemMotionCall::ItemImpulses { impulses } => {
            if let Some(error) = batch_guard("ItemImpulses", impulses.len()) {
                return error;
            }
            let deltas: Result<Vec<_>, _> = impulses
                .into_iter()
                .map(|(id, delta)| finite3(delta, "ItemImpulses.delta").map(|delta| (id, delta)))
                .collect();
            match deltas {
                Err(error) => error,
                Ok(deltas) => {
                    sim_query(|ctx| HostRet::Bools(ctx.world.impulse_item_entities(&deltas)))
                }
            }
        }
        ItemMotionCall::SteerItem { entity, vel } => {
            let vel = match vel.map(|v| finite3(v, "SteerItem.vel")).transpose() {
                Ok(vel) => vel,
                Err(error) => return error,
            };
            if vel.is_some_and(|v| {
                v.length() * crate::events::tick::TICK_DT
                    > petramond_world::collision::MAX_SAFE_EXTERNAL_SWEEP_DISTANCE
            }) {
                return HostRet::invalid("SteerItem: velocity exceeds the sweep bound".into());
            }
            sim_query(|ctx| {
                HostRet::Bool(
                    ctx.world
                        .dropped_items_mut()
                        .get_mut(entity)
                        .is_some_and(|item| item.steer(vel)),
                )
            })
        }
        ItemMotionCall::TakeItemEntity { entity } => sim_query(|ctx| {
            let stack = ctx.world.dropped_items().get(entity).map(|item| item.stack);
            if stack.is_some() {
                ctx.world.dropped_items_mut().remove(entity);
            }
            HostRet::ItemStack(stack.map(super::guards::item_stack_data))
        }),
    }
}
