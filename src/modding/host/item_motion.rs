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
    }
}
