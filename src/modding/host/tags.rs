//! Mob tag HostCalls: typed key/value pairs attached to live mob instances.
//!
//! Tags are namespaced like KV entries (caller must own the `mod_id:` prefix or
//! use the engine-reserved `petramond:` namespace), but they are typed and
//! visible to AI via [`AiMob::tags`](crate::mob::brain::AiMob).

use mod_api::{HostRet, MobTagLookup, MobTagOp, MobTagValue as ApiMobTagValue, TagCall};

use crate::mob::MobTagValue;

use super::entities::mob_snapshot;
use super::guards::{batch_guard, kv_write_guard, live_mob, sim_query, sim_read};

fn from_api(v: ApiMobTagValue) -> MobTagValue {
    MobTagValue::from(v)
}

pub(in crate::modding) fn to_api(v: &MobTagValue) -> ApiMobTagValue {
    ApiMobTagValue::from(v)
}

pub(super) fn handle_tag_call(mod_id: &str, call: TagCall) -> HostRet {
    match call {
        TagCall::MobTagGet { mob_id, key } => sim_read(|ctx| {
            let Some(mob) = live_mob(ctx, mob_id) else {
                return HostRet::MobTag(MobTagLookup::MissingMob);
            };
            let lookup = match mob.tags().get(&key) {
                Some(v) => MobTagLookup::Value(to_api(v)),
                None => MobTagLookup::Absent,
            };
            HostRet::MobTag(lookup)
        }),
        TagCall::MobTagSet { mob_id, key, value } => {
            let value_len = match &value {
                ApiMobTagValue::Bool(_) => 1,
                ApiMobTagValue::I64(_) | ApiMobTagValue::F64(_) => 8,
                ApiMobTagValue::Str(s) => s.len(),
            };
            match kv_write_guard(mod_id, &key, value_len) {
                Some(err) => err,
                None => sim_query(|ctx| {
                    let Some(mob) = live_mob(ctx, mob_id) else {
                        return HostRet::Bool(false);
                    };
                    // Presence transition = the mob_tag_added post event
                    // (value overwrites are silent — else the hot deadline
                    // rewrites would spam the queue).
                    let (kind, fresh) = (mob.kind, !mob.tags().contains_key(&key));
                    let value = from_api(value);
                    let set = ctx
                        .world
                        .mobs_mut()
                        .set_mob_tag(mob_id, key.clone(), value.clone());
                    if set && fresh {
                        ctx.queue.emit(crate::events::PostEvent::MobTagAdded {
                            id: mob_id,
                            kind,
                            key,
                            value,
                        });
                    }
                    HostRet::Bool(set)
                }),
            }
        }
        TagCall::MobTagDelete { mob_id, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                let Some(mob) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                let (kind, old) = (mob.kind, mob.tags().get(&key).cloned());
                let removed = ctx.world.mobs_mut().remove_mob_tag(mob_id, &key);
                if let (true, Some(value)) = (removed, old) {
                    ctx.queue.emit(crate::events::PostEvent::MobTagRemoved {
                        id: mob_id,
                        kind,
                        key,
                        value,
                    });
                }
                HostRet::Bool(removed)
            }),
        },
        TagCall::MobTagsGet { mob_id } => sim_read(|ctx| {
            let Some(mob) = live_mob(ctx, mob_id) else {
                return HostRet::MobTags(None);
            };
            HostRet::MobTags(Some(
                mob.tags()
                    .iter()
                    .map(|(k, v)| (k.clone(), to_api(v)))
                    .collect(),
            ))
        }),
        TagCall::MobsWithTag { key, value } => sim_read(|ctx| {
            let want = value.map(from_api);
            let mobs = ctx.world.mobs();
            HostRet::Mobs(
                mobs.with_tag(&key, want.as_ref())
                    .map(|(position, m)| mob_snapshot(position, m))
                    .collect(),
            )
        }),
        TagCall::MobTagsGetMany { mob_ids } => {
            if let Some(err) = batch_guard("MobTagsGetMany mob", mob_ids.len()) {
                return err;
            }
            sim_read(|ctx| HostRet::MobTagsMany(mob_ids.into_iter().map(|id| {
                let index = live_mob(ctx, id)?;
                ctx.world.mobs().mob_tags(index).map(|tags| {
                    tags.iter().map(|(k, v)| (k.clone(), to_api(v))).collect()
                })
            }).collect()))
        }
        TagCall::MobTagsWrite { writes } => {
            if let Some(err) = batch_guard("MobTagsWrite write", writes.len()) {
                return err;
            }
            let mut accepted = Vec::with_capacity(writes.len());
            for write in writes {
                let call = match write {
                    MobTagOp::Set { mob_id, key, value } => TagCall::MobTagSet { mob_id, key, value },
                    MobTagOp::Delete { mob_id, key } => TagCall::MobTagDelete { mob_id, key },
                };
                match handle_tag_call(mod_id, call) {
                    HostRet::Bool(ok) => accepted.push(ok),
                    err => return err,
                }
            }
            HostRet::Bools(accepted)
        }
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{calls, HostCall, HostRet, MobTagValue as Api};

    use crate::events::tick::TickEvents;
    use crate::events::{PostEvent, PostEventKind, PostQueue, RosterRefs, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::world::ServerWorld;
    use petramond_math::world_pos::WorldPos;

    /// The tag lifecycle events fire on PRESENCE TRANSITIONS through the ABI
    /// surface: a NEW key emits `mob_tag_added`, deleting a present key emits
    /// `mob_tag_removed` (carrying the evicted value) — while overwriting an
    /// existing key and deleting an absent one emit nothing.
    #[test]
    fn tag_presence_transitions_emit_post_events() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, WorldPos::new(1.0, 80.0, 1.0), 0.0));
        let id = world.mobs().instances()[0].id();

        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        queue.want_for_test(PostEventKind::MobTagAdded);
        queue.want_for_test(PostEventKind::MobTagRemoved);
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let set = |data: &mut ModStoreData, v: i64| {
                handle_host_call(
                    data,
                    HostCall::from(calls::MobTagSet {
                        mob_id: id,
                        key: "alpha:hunger".into(),
                        value: Api::I64(v),
                    }),
                )
            };
            let delete = |data: &mut ModStoreData| {
                handle_host_call(
                    data,
                    HostCall::from(calls::MobTagDelete {
                        mob_id: id,
                        key: "alpha:hunger".into(),
                    }),
                )
            };
            assert_eq!(set(&mut data, 3), HostRet::Bool(true), "fresh insert");
            assert_eq!(set(&mut data, 5), HostRet::Bool(true), "overwrite");
            assert_eq!(delete(&mut data), HostRet::Bool(true), "removal");
            assert_eq!(delete(&mut data), HostRet::Bool(false), "already absent");
        });
        let events = queue.take_events_for_test();
        assert_eq!(events.len(), 2, "one added + one removed: {events:?}");
        assert!(
            matches!(&events[0], PostEvent::MobTagAdded { id: eid, key, value, .. }
                if *eid == id && key == "alpha:hunger"
                    && *value == crate::mob::MobTagValue::Int(3)),
            "the fresh insert announces itself: {:?}",
            events[0]
        );
        assert!(
            matches!(&events[1], PostEvent::MobTagRemoved { id: eid, key, value, .. }
                if *eid == id && key == "alpha:hunger"
                    && *value == crate::mob::MobTagValue::Int(5)),
            "the removal carries the evicted value: {:?}",
            events[1]
        );

        // MobInfo: the single-mob snapshot answers live mobs and only them.
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            match handle_host_call(&mut data, HostCall::from(calls::MobInfo { mob_id: id })) {
                HostRet::Mob(Some(snap)) => assert_eq!(snap.id, id),
                other => panic!("live mob answers a snapshot, got {other:?}"),
            }
            assert_eq!(
                handle_host_call(&mut data, HostCall::from(calls::MobInfo { mob_id: id + 999 })),
                HostRet::Mob(None),
                "an unknown id is honestly absent"
            );
        });
    }
}
