use mod_api::{
    ActionRefusal, ActorCall, BlockRecord, DigProgress, EntityRef, HostRet, PlaceRequest,
};
use petramond_math::math::IVec3;
use petramond_world::construction::Record;

use super::construction::record_in;
use super::guards::{batch_guard, finite_pos, key_owned_by_namespace, sim_query};
use crate::events::{DeferredAction, SimCtx};
use crate::world::actor::PlaceCheck;

pub(super) fn handle_actor_call(mod_id: &str, call: ActorCall) -> HostRet {
    match call {
        ActorCall::ActorDig {
            actor,
            pos,
            tool_slot,
            collect,
        } => sim_query(|ctx| {
            HostRet::Dig(
                match dig(ctx, actor, IVec3::from_array(pos), tool_slot, collect) {
                    Ok(progress) => progress,
                    Err(refusal) => DigProgress::Refused(refusal),
                },
            )
        }),
        ActorCall::ActorPlace {
            actor,
            pos,
            record,
            pay,
        } => {
            let record = match payable_record(mod_id, &record, pay) {
                Ok(record) => record,
                Err(refusal) => return HostRet::Place(PlaceRequest::Refused(refusal)),
            };
            sim_query(|ctx| HostRet::Place(place(ctx, actor, IVec3::from_array(pos), record, pay)))
        }
        ActorCall::ActorInteract { actor, pos } => sim_query(|ctx| {
            let EntityRef::Mob(mob_id) = actor else {
                return HostRet::Bool(false);
            };
            let pos = IVec3::from_array(pos);
            let reached = ctx
                .world
                .actor(mob_id)
                .is_ok_and(|a| ctx.world.block_click(&a, pos).is_ok());
            if !reached {
                return HostRet::Bool(false);
            }
            ctx.queue
                .push_action(DeferredAction::ActorInteract { mob_id, pos });
            HostRet::Bool(true)
        }),
        ActorCall::ActorPlaceCheck {
            actor,
            from,
            pos,
            record,
            pay,
        } => {
            let record = match payable_record(mod_id, &record, pay) {
                Ok(record) => record,
                Err(refusal) => return HostRet::Place(PlaceRequest::Refused(refusal)),
            };
            let from = match finite_pos(from, "ActorPlaceCheck from") {
                Ok(from) => from,
                Err(err) => return err,
            };
            sim_query(|ctx| {
                let EntityRef::Mob(mob_id) = actor else {
                    return HostRet::Place(PlaceRequest::Refused(ActionRefusal::NoActor));
                };
                let actor = match ctx.world.actor(mob_id) {
                    Ok(actor) => actor,
                    Err(refusal) => return HostRet::Place(PlaceRequest::Refused(refusal)),
                };
                let standing = actor.standing_at(from);
                HostRet::Place(judge(ctx, &standing, IVec3::from_array(pos), &record, pay))
            })
        }
        ActorCall::ActorAims {
            actor,
            from,
            pos,
            record,
        } => {
            if let Some(err) = batch_guard("ActorAims position", from.len()) {
                return err;
            }
            let record = match record.as_ref().map(record_in) {
                Some(Err(_)) => {
                    return HostRet::Aims(vec![Err(ActionRefusal::Unsupported); from.len()])
                }
                Some(Ok(record)) => Some(record),
                None => None,
            };
            let pos = IVec3::from_array(pos);
            let feet = match feet_of(&from, "ActorAims from") {
                Ok(feet) => feet,
                Err(err) => return err,
            };
            sim_query(|ctx| {
                let EntityRef::Mob(mob_id) = actor else {
                    return HostRet::Aims(vec![Err(ActionRefusal::NoActor); from.len()]);
                };
                let actor = match ctx.world.actor(mob_id) {
                    Ok(actor) => actor,
                    Err(refusal) => return HostRet::Aims(vec![Err(refusal); from.len()]),
                };
                let aims = match &record {
                    Some(record) => ctx.world.placement_aims(&actor, &feet, pos, record),
                    None => feet
                        .iter()
                        .map(|&feet| ctx.world.block_click(&actor.standing_at(feet), pos))
                        .collect(),
                };
                HostRet::Aims(
                    aims.into_iter()
                        .map(|aim| aim.map(|at| [at.x, at.y, at.z]))
                        .collect(),
                )
            })
        }
    }
}

fn feet_of(
    from: &[[f64; 3]],
    what: &str,
) -> Result<Vec<petramond_math::world_pos::WorldPos>, HostRet> {
    from.iter().map(|&feet| finite_pos(feet, what)).collect()
}

fn payable_record(mod_id: &str, record: &BlockRecord, pay: bool) -> Result<Record, ActionRefusal> {
    let record = record_in(record).map_err(|_| ActionRefusal::Unsupported)?;
    let owned = petramond_world::registry::names()
        .blocks
        .name(record.block.id())
        .is_some_and(|name| key_owned_by_namespace(mod_id, name));
    if pay || owned {
        Ok(record)
    } else {
        Err(ActionRefusal::NotOwned)
    }
}

fn dig(
    ctx: &mut SimCtx<'_>,
    actor: EntityRef,
    pos: IVec3,
    tool_slot: Option<u32>,
    collect: bool,
) -> Result<DigProgress, ActionRefusal> {
    let EntityRef::Mob(mob_id) = actor else {
        return Err(ActionRefusal::NoActor);
    };
    let actor = ctx.world.actor(mob_id)?;
    let target = ctx.world.dig_check(&actor, pos, tool_slot)?;
    let now = ctx.world.current_tick();
    let tool = target.tool.and_then(|t| t.tool());
    let step = ctx
        .world
        .mobs_mut()
        .advance_dig(actor.id, now, pos, target.block, tool)
        .ok_or(ActionRefusal::NoActor)?;
    Ok(match step {
        crate::mob::DigStep::Digging(progress) => DigProgress::Digging { progress },
        crate::mob::DigStep::Done(_) => {
            ctx.queue.push_action(DeferredAction::ActorBreak {
                mob_id,
                pos,
                target,
                tool_slot,
                collect,
            });
            DigProgress::Breaking
        }
    })
}

fn place(
    ctx: &mut SimCtx<'_>,
    actor: EntityRef,
    pos: IVec3,
    record: Record,
    pay: bool,
) -> PlaceRequest {
    let EntityRef::Mob(mob_id) = actor else {
        return PlaceRequest::Refused(ActionRefusal::NoActor);
    };
    let actor = match ctx.world.actor(mob_id) {
        Ok(actor) => actor,
        Err(refusal) => return PlaceRequest::Refused(refusal),
    };
    match judge(ctx, &actor, pos, &record, pay) {
        PlaceRequest::Queued => {
            ctx.queue.push_action(DeferredAction::ActorPlace {
                mob_id,
                pos,
                record,
                pay,
            });
            PlaceRequest::Queued
        }
        answer => answer,
    }
}

fn judge(
    ctx: &SimCtx<'_>,
    actor: &crate::world::actor::Actor,
    pos: IVec3,
    record: &Record,
    pay: bool,
) -> PlaceRequest {
    let world = &*ctx.world;
    let check = world.place_check(actor, pos, record, pay, &mut |cell, boxes| {
        world.body_in_the_way(cell, boxes)
    });
    match check {
        PlaceCheck::Satisfied => PlaceRequest::Satisfied,
        PlaceCheck::Refused(refusal) => PlaceRequest::Refused(refusal),
        PlaceCheck::Ready(_) => PlaceRequest::Queued,
    }
}
