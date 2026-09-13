//! Body-condition primitives. Applying never deals damage outside the tick.

use mod_api::{EntityRef, HostCall, HostRet};
use petramond_world::condition::ConditionDef;
use petramond_world::exposure::BodyExposure;

use super::guards::{live_mob, sim_mutating_query};

fn with_exposure<R>(
    ctx: &mut crate::events::SimCtx<'_>,
    entity: EntityRef,
    f: impl FnOnce(&mut BodyExposure) -> R,
) -> Option<R> {
    match entity {
        EntityRef::Player(id) => ctx
            .with_player(crate::player::PlayerId(id.0), |p| {
                (!p.is_spectator() && p.health() > 0).then(|| f(p.exposure_mut()))
            })
            .flatten(),
        EntityRef::Mob(id) => {
            let index = live_mob(ctx, id)?;
            Some(f(ctx.world.mobs_mut().exposure_mut(index)?))
        }
    }
}

fn condition(call: &str, id: mod_api::ConditionId) -> Result<&'static ConditionDef, HostRet> {
    petramond_world::condition::defs()
        .get(id.0 as usize)
        .ok_or_else(|| HostRet::Error(format!("{call}: unregistered condition id {}", id.0)))
}

pub(super) fn handle(call: HostCall) -> HostRet {
    match call {
        HostCall::EntityConditionApply {
            entity,
            condition: id,
            stage,
            ticks,
        } => {
            let def = match condition("EntityConditionApply", id) {
                Ok(def) => def,
                Err(err) => return err,
            };
            if stage as usize >= def.stages.len() {
                return HostRet::Error(format!(
                    "EntityConditionApply: condition '{}' has no stage {stage}",
                    def.name
                ));
            }
            sim_mutating_query(|ctx| {
                HostRet::Bool(
                    with_exposure(ctx, entity, |e| e.apply(def, stage, ticks)).unwrap_or(false),
                )
            })
        }
        HostCall::EntityConditionCool {
            entity,
            condition: id,
            ticks,
        } => {
            let def = match condition("EntityConditionCool", id) {
                Ok(def) => def,
                Err(err) => return err,
            };
            sim_mutating_query(|ctx| {
                HostRet::Bool(with_exposure(ctx, entity, |e| e.cool(def.id, ticks)).is_some())
            })
        }
        _ => unreachable!(),
    }
}
