//! Mob tag HostCalls: typed key/value pairs attached to live mob instances.
//!
//! Tags are namespaced like KV entries (caller must own the `mod_id:` prefix or
//! use the engine-reserved `petramond:` namespace), but they are typed and
//! visible to AI via [`AiMob::tags`](crate::mob::brain::AiMob).

use mod_api::{HostCall, HostRet, MobTagLookup, MobTagValue as ApiMobTagValue};

use crate::mob::MobTagValue;

use super::entities::mob_snapshot;
use super::guards::{kv_write_guard, live_mob, sim_query};

pub(super) fn from_api(v: ApiMobTagValue) -> MobTagValue {
    match v {
        ApiMobTagValue::Bool(b) => MobTagValue::Bool(b),
        ApiMobTagValue::I64(i) => MobTagValue::Int(i),
        ApiMobTagValue::F64(f) => MobTagValue::Float(f),
        ApiMobTagValue::Str(s) => MobTagValue::String(s),
    }
}

fn to_api(v: &MobTagValue) -> ApiMobTagValue {
    match v {
        MobTagValue::Bool(b) => ApiMobTagValue::Bool(*b),
        MobTagValue::Int(i) => ApiMobTagValue::I64(*i),
        MobTagValue::Float(f) => ApiMobTagValue::F64(*f),
        MobTagValue::String(s) => ApiMobTagValue::Str(s.clone()),
    }
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
                    HostRet::Bool(ctx.world.mobs_mut().set_mob_tag(index, key, from_api(value)))
                }),
            }
        }
        HostCall::MobTagDelete { mob_id, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                HostRet::Bool(ctx.world.mobs_mut().remove_mob_tag(index, &key))
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
