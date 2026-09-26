//! Body-condition primitives. Applying never deals damage outside the tick.

use mod_api::{ConditionCall, EntityRef, HostRet};
use petramond_world::condition::ConditionDef;
use petramond_world::exposure::BodyExposure;

use super::guards::{batch_guard, live_mob, sim_query};

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
            live_mob(ctx, id)?;
            Some(f(ctx.world.mobs_mut().exposure_mut(id)?))
        }
    }
}

fn condition(call: &str, id: mod_api::ConditionId) -> Result<&'static ConditionDef, HostRet> {
    petramond_world::condition::defs()
        .get(id.0 as usize)
        .ok_or_else(|| HostRet::invalid(format!("{call}: unregistered condition id {}", id.0)))
}

pub(super) fn handle(call: ConditionCall) -> HostRet {
    match call {
        ConditionCall::EntityConditionApply {
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
                return HostRet::invalid(format!(
                    "EntityConditionApply: condition '{}' has no stage {stage}",
                    def.name
                ));
            }
            sim_query(|ctx| {
                HostRet::Bool(
                    with_exposure(ctx, entity, |e| e.apply(def, stage, ticks)).unwrap_or(false),
                )
            })
        }
        ConditionCall::EntityConditionCool {
            entity,
            condition: id,
            ticks,
        } => {
            let def = match condition("EntityConditionCool", id) {
                Ok(def) => def,
                Err(err) => return err,
            };
            sim_query(|ctx| {
                HostRet::Bool(with_exposure(ctx, entity, |e| e.cool(def.id, ticks)).is_some())
            })
        }
        ConditionCall::EntityConditionsMany { ops } => {
            if let Some(err) = batch_guard("EntityConditionsMany command", ops.len()) {
                return err;
            }
            let mut accepted = Vec::with_capacity(ops.len());
            for op in ops {
                let call = match op {
                    mod_api::ConditionOp::Apply {
                        entity,
                        condition,
                        stage,
                        ticks,
                    } => ConditionCall::EntityConditionApply {
                        entity,
                        condition,
                        stage,
                        ticks,
                    },
                    mod_api::ConditionOp::Cool {
                        entity,
                        condition,
                        ticks,
                    } => ConditionCall::EntityConditionCool {
                        entity,
                        condition,
                        ticks,
                    },
                };
                match handle(call) {
                    HostRet::Bool(ok) => accepted.push(ok),
                    err => return err,
                }
            }
            HostRet::Bools(accepted)
        }
    }
}
