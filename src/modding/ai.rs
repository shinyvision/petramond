//! Scripted (WASM) mob-AI node dispatch — the session registry
//! `mob::behavior::wasm`'s nodes resolve through.
//!
//! Mirrors `gen::install` in spirit but stays THREAD-LOCAL: mob AI runs only
//! on the SIM thread (the deterministic game tick — the server
//! thread). Keeping the registry per-thread (instead of a
//! process-wide map) preserves test isolation: parallel test sessions each
//! install into their own thread. The server thread re-installs the session's
//! map on startup via `ModHost::install_thread_ai_nodes`. A dispatch from
//! a thread without an install simply finds no registration and decides
//! nothing.
//!
//! Dispatch is BATCHED: the mob manager gathers every scripted node's
//! request across the live population in one serial phase of its tick and
//! hands them to [`dispatch_batch`], which makes ONE guest call per node key
//! ([`GuestCall::AiNodeBatch`]) with every mob's context, in live-set order.
//! The contexts are serialized straight from the mobs ([`AiNodeCtxRef`]): a
//! mob's tag map rides its shared handle, so no tag string is copied on the
//! host. A guest predating the batch call (before ABI 2.1) declines it once and is
//! served one [`GuestCall::AiNode`] per mob from then on.
//!
//! Dispatch is DETACHED — no simulation scope is published — because it runs
//! mid-mob-tick, where the world is immutably borrowed. Sim host calls made
//! by the guest error (decision-only contract, see `GuestCall::AiNode`); the
//! core calls work, `CurrentTick` included: the dispatcher publishes the
//! tick it snapshotted into `AiNodeCtx` ([`detached_tick`]) so the tick
//! clock never needs the sim scope here.

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
    /// The game tick of the in-flight detached AI dispatch — what
    /// `CoreCall::CurrentTick` reads when no sim scope is active.
    static DETACHED_TICK: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Install the session's node map on THIS thread (empty or not — installing
/// always is what evicts a previous session's registrations). Called from
/// `ModHost::initialize` on the constructing thread and again by the server
/// thread at startup (`ModHost::install_thread_ai_nodes`).
pub(super) fn install(map: HashMap<String, AiNodeRegistration>) {
    INSTALLED.with(|cell| *cell.borrow_mut() = map);
}

/// Whether `key` has a live registration on this thread — the pre-gather
/// gate that lets an unclaimed scripted node (mod disabled, mid-load) skip
/// building its request entirely.
pub fn is_claimed(key: &str) -> bool {
    INSTALLED.with(|cell| cell.borrow().contains_key(key))
}

/// The tick published for the current detached AI dispatch, if one is in
/// flight on this thread. Read by the `CurrentTick` host-call handler as its
/// scope-free fallback.
pub fn detached_tick() -> Option<u64> {
    DETACHED_TICK.with(Cell::get)
}

/// Publish `tick` as this thread's detached-dispatch tick for the duration of
/// `f` — wrapped around every guest AI call by [`dispatch_batch`].
pub fn with_detached_tick<T>(tick: u64, f: impl FnOnce() -> T) -> T {
    DETACHED_TICK.with(|t| t.set(Some(tick)));
    let out = f();
    DETACHED_TICK.with(|t| t.set(None));
    out
}

/// One mob's request to one scripted node this tick: the node's key and the
/// mob's [`AiNodeCtx`] contents, already in ABI vocabulary — except the tag
/// map, which stays the mob's own shared handle until it is serialized.
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
}

impl AiNodeRequest {
    /// The owned context for the per-mob fallback (a guest older than ABI 2.1) — the
    /// one place a request's tags are copied.
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

/// A request viewed as the [`AiNodeCtx`] it stands for: serializes to exactly
/// the bytes the owned context would (postcard encodes struct fields
/// positionally and enum variants by index), borrowing the tag map.
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
        ctx.serialize_field("tags", &AbiTags(&r.tags))?;
        ctx.end()
    }
}

/// A tag map serialized as the ABI's `Vec<(String, mod_api::MobTagValue)>`.
struct AbiTags<'a>(&'a BTreeMap<String, MobTagValue>);

impl Serialize for AbiTags<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|(k, v)| (k.as_str(), AbiTag(v))))
    }
}

/// One engine tag value serialized as the ABI's `mod_api::MobTagValue`
/// (same variant order: `Bool`, `I64`, `F64`, `Str`).
struct AbiTag<'a>(&'a MobTagValue);

impl Serialize for AbiTag<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            MobTagValue::Bool(b) => serializer.serialize_newtype_variant("MobTagValue", 0, "Bool", b),
            MobTagValue::Int(i) => serializer.serialize_newtype_variant("MobTagValue", 1, "I64", i),
            MobTagValue::Float(f) => serializer.serialize_newtype_variant("MobTagValue", 2, "F64", f),
            MobTagValue::String(s) => {
                serializer.serialize_newtype_variant("MobTagValue", 3, "Str", s.as_str())
            }
        }
    }
}

/// Declaration index of [`GuestCall::AiNodeBatch`] — pinned against the owned
/// encoding by this module's tests.
const AI_NODE_BATCH_VARIANT: u32 = 17;

/// A batch of borrowed contexts serialized as [`GuestCall::AiNodeBatch`].
#[derive(Debug)]
struct AiNodeBatchRef<'a> {
    callback_id: u32,
    ctxs: &'a [AiNodeCtxRef<'a>],
}

impl Serialize for AiNodeBatchRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut call = serializer.serialize_struct_variant(
            "GuestCall",
            AI_NODE_BATCH_VARIANT,
            "AiNodeBatch",
            2,
        )?;
        call.serialize_field("callback_id", &self.callback_id)?;
        call.serialize_field("ctxs", self.ctxs)?;
        call.end()
    }
}

/// The call kind a batch encodes as, for the instance's declined-call memory.
fn batch_kind() -> std::mem::Discriminant<GuestCall> {
    std::mem::discriminant(&GuestCall::AiNodeBatch {
        callback_id: 0,
        ctxs: Vec::new(),
    })
}

/// One decision per request, in request order: every key's requests go to
/// its node in ONE guest call, keys in order of first appearance. `None` =
/// no opinion — an unclaimed key (mod never claimed it, disabled, or
/// mid-load), a disabled mod, or a node that answered nothing for that mob;
/// exactly like an engine node returning defaults.
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

/// One node's replies for `members` (indices into `requests`), in order.
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
        let call = AiNodeBatchRef {
            callback_id: reg.callback_id,
            ctxs: &ctxs,
        };
        match instance.call_guest_encoded(&call, batch_kind(), &call) {
            Some(GuestRet::AiDecisions(decisions)) if decisions.len() == members.len() => {
                return decisions;
            }
            Some(_) => {
                instance.disable(
                    "answered an AI node batch with a reply of the wrong shape or length",
                );
                return vec![None; members.len()];
            }
            None if instance.disabled() || !instance.declines(batch_kind()) => {
                return vec![None; members.len()];
            }
            // Declined just now: an older guest — serve it per mob below.
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
        }
    }

    /// The borrowed batch must be byte-for-byte the owned `AiNodeBatch` —
    /// the guest decodes the ordinary ABI type from it.
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
        let borrowed = mod_api::encode(&AiNodeBatchRef {
            callback_id: 5,
            ctxs: &ctxs,
        })
        .unwrap();
        let owned = mod_api::encode(&GuestCall::AiNodeBatch {
            callback_id: 5,
            ctxs: requests.iter().map(|r| r.to_ctx(77)).collect(),
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
