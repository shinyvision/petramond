//! Mob tag HostCalls: typed key/value pairs attached to live mob instances.
//!
//! Tags are namespaced like KV entries (caller must own the `mod_id:` prefix or
//! use the engine-reserved `petramond:` namespace), but they are typed and
//! visible to AI via [`AiMob::tags`](crate::mob::brain::AiMob).

use mod_api::{HostCall, HostRet, MobTagLookup, MobTagValue as ApiMobTagValue};

use crate::mob::MobTagValue;

use super::entities::mob_snapshot;
use super::guards::{kv_write_guard, live_mob, sim_query};

fn from_api(v: ApiMobTagValue) -> MobTagValue {
    MobTagValue::from(v)
}

pub(in crate::modding) fn to_api(v: &MobTagValue) -> ApiMobTagValue {
    ApiMobTagValue::from(v)
}

pub(super) fn handle_tag_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::MobTagGet { mob_id, key } => sim_query(|ctx| {
            let Some(index) = live_mob(ctx, mob_id) else {
                return HostRet::MobTag(MobTagLookup::MissingMob);
            };
            let lookup = match ctx.world.mobs().mob_tag(index, &key) {
                Some(v) => MobTagLookup::Value(to_api(v)),
                None => MobTagLookup::Absent,
            };
            HostRet::MobTag(lookup)
        }),
        HostCall::MobTagSet { mob_id, key, value } => {
            let value_len = match &value {
                ApiMobTagValue::Bool(_) => 1,
                ApiMobTagValue::I64(_) | ApiMobTagValue::F64(_) => 8,
                ApiMobTagValue::Str(s) => s.len(),
            };
            match kv_write_guard(mod_id, &key, value_len) {
                Some(err) => err,
                None => sim_query(|ctx| {
                    let Some(index) = live_mob(ctx, mob_id) else {
                        return HostRet::Bool(false);
                    };
                    // Presence transition = the mob_tag_added post event
                    // (value overwrites are silent — else the hot deadline
                    // rewrites would spam the queue).
                    let fresh = ctx.world.mobs().mob_tag(index, &key).is_none();
                    let value = from_api(value);
                    let set = ctx
                        .world
                        .mobs_mut()
                        .set_mob_tag(index, key.clone(), value.clone());
                    if set && fresh {
                        let kind = ctx.world.mobs().instances()[index].kind;
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
        HostCall::MobTagDelete { mob_id, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                let old = ctx.world.mobs().mob_tag(index, &key).cloned();
                let removed = ctx.world.mobs_mut().remove_mob_tag(index, &key);
                if let (true, Some(value)) = (removed, old) {
                    let kind = ctx.world.mobs().instances()[index].kind;
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
        HostCall::MobTagsGet { mob_id } => sim_query(|ctx| {
            let Some(index) = live_mob(ctx, mob_id) else {
                return HostRet::MobTags(None);
            };
            HostRet::MobTags(ctx.world.mobs().mob_tags(index).map(|tags| {
                tags.iter()
                    .map(|(k, v)| (k.clone(), to_api(v)))
                    .collect()
            }))
        }),
        HostCall::MobsWithTag { key, value } => sim_query(|ctx| {
            let want = value.map(from_api);
            let mobs = ctx.world.mobs();
            HostRet::Mobs(
                mobs.indices_with_tag(&key, want.as_ref())
                    .into_iter()
                    .map(|i| mob_snapshot(i, &mobs.instances()[i]))
                    .collect(),
            )
        }),
        other => HostRet::Error(format!(
            "non-tag call {other:?} mis-routed to handle_tag_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet, MobTagValue as Api};

    use crate::events::{PostEvent, PostEventKind, PostQueue, SimCtx};
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    /// The tag lifecycle events fire on PRESENCE TRANSITIONS through the ABI
    /// surface: a NEW key emits `mob_tag_added`, deleting a present key emits
    /// `mob_tag_removed` (carrying the evicted value) — while overwriting an
    /// existing key and deleting an absent one emit nothing.
    #[test]
    fn tag_presence_transitions_emit_post_events() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(1.0, 80.0, 1.0), 0.0));
        let id = world.mobs().instances()[0].id();

        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        queue.want_for_test(PostEventKind::MobTagAdded);
        queue.want_for_test(PostEventKind::MobTagRemoved);
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let set = |data: &mut ModStoreData, v: i64| {
                handle_host_call(
                    data,
                    HostCall::MobTagSet {
                        mob_id: id,
                        key: "alpha:hunger".into(),
                        value: Api::I64(v),
                    },
                )
            };
            let delete = |data: &mut ModStoreData| {
                handle_host_call(
                    data,
                    HostCall::MobTagDelete {
                        mob_id: id,
                        key: "alpha:hunger".into(),
                    },
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
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            match handle_host_call(&mut data, HostCall::MobInfo { mob_id: id }) {
                HostRet::Mob(Some(snap)) => assert_eq!(snap.id, id),
                other => panic!("live mob answers a snapshot, got {other:?}"),
            }
            assert_eq!(
                handle_host_call(&mut data, HostCall::MobInfo { mob_id: id + 999 }),
                HostRet::Mob(None),
                "an unknown id is honestly absent"
            );
        });
    }
}
