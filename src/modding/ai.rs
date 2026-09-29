use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use mod_api::{AiNodeCtx, AiNodeDecision, GuestCall, GuestRet};
use serde::ser::{SerializeStruct, SerializeStructVariant, Serializer};
use serde::Serialize;

use super::SharedInstance;
use crate::mob::MobTagValue;

#[derive(Clone)]
pub(super) struct AiNodeRegistration {
    pub instance: SharedInstance,
    pub callback_id: u32,
}

thread_local! {
    static INSTALLED: RefCell<HashMap<String, AiNodeRegistration>> =
        RefCell::new(HashMap::new());
    static DETACHED_TICK: Cell<Option<u64>> = const { Cell::new(None) };
}

pub(super) fn install(map: HashMap<String, AiNodeRegistration>) {
    INSTALLED.with(|cell| *cell.borrow_mut() = map);
}

pub fn is_claimed(key: &str) -> bool {
    INSTALLED.with(|cell| cell.borrow().contains_key(key))
}

pub fn detached_tick() -> Option<u64> {
    DETACHED_TICK.with(Cell::get)
}

pub fn with_detached_tick<T>(tick: u64, f: impl FnOnce() -> T) -> T {
    DETACHED_TICK.with(|t| t.set(Some(tick)));
    let out = f();
    DETACHED_TICK.with(|t| t.set(None));
    out
}

#[derive(Clone, Debug)]
pub struct AiNodeRequest {
    pub key: &'static str,
    pub mob_id: u64,
    pub pos: [f64; 3],
    pub cell: [i32; 3],
    pub yaw: f32,
    pub player_id: mod_api::PlayerId,
    pub player_pos: [f64; 3],
    pub nav_idle: bool,
    pub in_fluid: Option<mod_api::BlockId>,
    pub target: Option<mod_api::EntityRef>,
    pub attacker: Option<(mod_api::EntityRef, u32)>,
    pub player_held: Option<mod_api::ItemId>,
    pub player_foothold: Option<[i32; 3]>,
    pub tags: Arc<BTreeMap<String, MobTagValue>>,
    /// The tag map's revision ([`crate::mob::Instance::tags_rev`]): a batch resends a mob's tags
    /// only when it moved.
    pub tags_rev: u64,
}

impl AiNodeRequest {
    fn to_ctx(&self, tick: u64) -> AiNodeCtx {
        AiNodeCtx {
            mob_id: self.mob_id,
            pos: self.pos,
            cell: self.cell,
            yaw: self.yaw,
            tick,
            player_id: self.player_id,
            player_pos: self.player_pos,
            nav_idle: self.nav_idle,
            in_fluid: self.in_fluid,
            target: self.target,
            attacker: self.attacker,
            player_held: self.player_held,
            player_foothold: self.player_foothold,
            tags: self
                .tags
                .iter()
                .map(|(k, v)| (k.clone(), mod_api::MobTagValue::from(v)))
                .collect(),
        }
    }
}

#[derive(Debug)]
pub struct AiNodeCtxRef<'a> {
    request: &'a AiNodeRequest,
    tick: u64,
}

impl Serialize for AiNodeCtxRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let r = self.request;
        let mut ctx = serializer.serialize_struct("AiNodeCtx", 14)?;
        ctx.serialize_field("mob_id", &r.mob_id)?;
        ctx.serialize_field("pos", &r.pos)?;
        ctx.serialize_field("cell", &r.cell)?;
        ctx.serialize_field("yaw", &r.yaw)?;
        ctx.serialize_field("tick", &self.tick)?;
        ctx.serialize_field("player_id", &r.player_id)?;
        ctx.serialize_field("player_pos", &r.player_pos)?;
        ctx.serialize_field("nav_idle", &r.nav_idle)?;
        ctx.serialize_field("in_fluid", &r.in_fluid)?;
        ctx.serialize_field("target", &r.target)?;
        ctx.serialize_field("attacker", &r.attacker)?;
        ctx.serialize_field("player_held", &r.player_held)?;
        ctx.serialize_field("player_foothold", &r.player_foothold)?;
        // A batch carries tags beside the contexts, only for mobs whose tags changed.
        ctx.serialize_field("tags", &AbiTags(&EMPTY_TAGS))?;
        ctx.end()
    }
}

static EMPTY_TAGS: BTreeMap<String, MobTagValue> = BTreeMap::new();

#[derive(Debug)]
struct AbiTags<'a>(&'a BTreeMap<String, MobTagValue>);

impl Serialize for AbiTags<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|(k, v)| (k.as_str(), AbiTag(v))))
    }
}

#[derive(Debug)]
struct AbiTag<'a>(&'a MobTagValue);

impl Serialize for AbiTag<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            MobTagValue::Bool(b) => {
                serializer.serialize_newtype_variant("MobTagValue", 0, "Bool", b)
            }
            MobTagValue::Int(i) => serializer.serialize_newtype_variant("MobTagValue", 1, "I64", i),
            MobTagValue::Float(f) => {
                serializer.serialize_newtype_variant("MobTagValue", 2, "F64", f)
            }
            MobTagValue::String(s) => {
                serializer.serialize_newtype_variant("MobTagValue", 3, "Str", s.as_str())
            }
        }
    }
}

const AI_NODE_BATCH_VARIANT: u32 = 17;

#[derive(Debug)]
struct AiNodeBatchRef<'a> {
    callback_id: u32,
    ctxs: &'a [AiNodeCtxRef<'a>],
    /// Parallel to `ctxs`: the mob's tags when they changed since this instance last saw it.
    tags: &'a [Option<AbiTags<'a>>],
}

impl Serialize for AiNodeBatchRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut call = serializer.serialize_struct_variant(
            "GuestCall",
            AI_NODE_BATCH_VARIANT,
            "AiNodeBatch",
            3,
        )?;
        call.serialize_field("callback_id", &self.callback_id)?;
        call.serialize_field("ctxs", self.ctxs)?;
        call.serialize_field("tags", self.tags)?;
        call.end()
    }
}

/// Per AI node, the tag revision last sent for each mob of its latest batch.
pub(super) type SentTags = rustc_hash::FxHashMap<u32, rustc_hash::FxHashMap<u64, u64>>;

fn batch_kind() -> std::mem::Discriminant<GuestCall> {
    std::mem::discriminant(&GuestCall::AiNodeBatch {
        callback_id: 0,
        ctxs: Vec::new(),
        tags: Vec::new(),
    })
}

pub fn dispatch_batch(tick: u64, requests: &[AiNodeRequest]) -> Vec<Option<AiNodeDecision>> {
    let mut out = vec![None; requests.len()];
    let mut keys: Vec<&'static str> = Vec::new();
    for r in requests {
        if !keys.contains(&r.key) {
            keys.push(r.key);
        }
    }
    INSTALLED.with(|cell| {
        let map = cell.borrow();
        for key in keys {
            let Some(reg) = map.get(key) else {
                continue;
            };
            let members: Vec<usize> = (0..requests.len())
                .filter(|&i| requests[i].key == key)
                .collect();
            let replies = with_detached_tick(tick, || dispatch_node(reg, tick, requests, &members));
            for (i, reply) in members.into_iter().zip(replies) {
                out[i] = reply;
            }
        }
    });
    out
}

fn dispatch_node(
    reg: &AiNodeRegistration,
    tick: u64,
    requests: &[AiNodeRequest],
    members: &[usize],
) -> Vec<Option<AiNodeDecision>> {
    let mut instance = reg.instance.lock().unwrap();
    if !instance.declines(batch_kind()) {
        let ctxs: Vec<AiNodeCtxRef<'_>> = members
            .iter()
            .map(|&i| AiNodeCtxRef {
                request: &requests[i],
                tick,
            })
            .collect();
        // Both sides keep exactly the mobs of the latest batch: a mob's tags go over when its
        // revision differs from the one last sent, or when it was not in the last batch.
        let last = instance
            .sent_tags()
            .remove(&reg.callback_id)
            .unwrap_or_default();
        let mut next = rustc_hash::FxHashMap::default();
        let tags: Vec<Option<AbiTags<'_>>> = members
            .iter()
            .map(|&i| {
                let r = &requests[i];
                next.insert(r.mob_id, r.tags_rev);
                (last.get(&r.mob_id) != Some(&r.tags_rev)).then_some(AbiTags(&r.tags))
            })
            .collect();
        instance.sent_tags().insert(reg.callback_id, next);
        let call = AiNodeBatchRef {
            callback_id: reg.callback_id,
            ctxs: &ctxs,
            tags: &tags,
        };
        match instance.call_guest_encoded(
            &call,
            batch_kind(),
            super::watchdog::CallClass::Ai,
            &call,
        ) {
            Some(GuestRet::AiDecisions(decisions)) if decisions.len() == members.len() => {
                return decisions;
            }
            Some(_) => {
                instance
                    .disable("answered an AI node batch with a reply of the wrong shape or length");
                return vec![None; members.len()];
            }
            None if instance.disabled() || !instance.declines(batch_kind()) => {
                return vec![None; members.len()];
            }
            None => {}
        }
    }
    members
        .iter()
        .map(|&i| {
            let call = GuestCall::AiNode {
                callback_id: reg.callback_id,
                ctx: requests[i].to_ctx(tick),
            };
            match instance.call_guest_detached(&call)? {
                GuestRet::AiDecision(decision) => decision,
                _ => {
                    instance.disable("returned a non-decision reply to an AI node dispatch");
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(mob_id: u64, tags: &[(&str, MobTagValue)]) -> AiNodeRequest {
        AiNodeRequest {
            key: "m:node",
            mob_id,
            pos: [1.0, 2.0, 3.0],
            cell: [1, 2, 3],
            yaw: 0.5,
            player_id: mod_api::PlayerId(2),
            player_pos: [4.0, 5.0, 6.0],
            nav_idle: true,
            in_fluid: Some(mod_api::BlockId(3)),
            target: Some(mod_api::EntityRef::Mob(8)),
            attacker: Some((mod_api::EntityRef::Player(mod_api::PlayerId(2)), 3)),
            player_held: Some(mod_api::ItemId(7)),
            player_foothold: Some([4, 5, 6]),
            tags: Arc::new(
                tags.iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone()))
                    .collect(),
            ),
            tags_rev: 1,
        }
    }

    #[test]
    fn a_borrowed_batch_encodes_exactly_like_the_owned_call() {
        let requests = [
            request(
                1,
                &[
                    ("m:a", MobTagValue::Bool(true)),
                    ("m:b", MobTagValue::Int(-3)),
                    ("m:c", MobTagValue::Float(0.25)),
                    ("m:d", MobTagValue::String("hay".into())),
                ],
            ),
            request(9, &[]),
        ];
        let ctxs: Vec<AiNodeCtxRef<'_>> = requests
            .iter()
            .map(|request| AiNodeCtxRef { request, tick: 77 })
            .collect();
        let tags: Vec<Option<AbiTags<'_>>> = requests
            .iter()
            .enumerate()
            .map(|(i, r)| (i == 0).then_some(AbiTags(&r.tags)))
            .collect();
        let borrowed = mod_api::encode(&AiNodeBatchRef {
            callback_id: 5,
            ctxs: &ctxs,
            tags: &tags,
        })
        .unwrap();
        let owned = mod_api::encode(&GuestCall::AiNodeBatch {
            callback_id: 5,
            ctxs: requests
                .iter()
                .map(|r| AiNodeCtx {
                    tags: Vec::new(),
                    ..r.to_ctx(77)
                })
                .collect(),
            tags: vec![Some(requests[0].to_ctx(77).tags), None],
        })
        .unwrap();
        assert_eq!(borrowed, owned);
        let decoded: GuestCall = mod_api::decode(&borrowed).unwrap();
        assert_eq!(
            std::mem::discriminant(&decoded),
            batch_kind(),
            "the batch decodes as AiNodeBatch"
        );
    }

    #[test]
    fn unclaimed_keys_answer_no_opinion_per_request() {
        install(HashMap::new());
        let requests = [request(1, &[]), request(2, &[])];
        assert_eq!(dispatch_batch(3, &requests), vec![None, None]);
        assert_eq!(detached_tick(), None, "no dispatch leaves a tick published");
    }
}
