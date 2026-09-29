use core::cell::UnsafeCell;

use mod_api::{Decoded, GuestCall, GuestRet, HostCall, HostRet};

use crate::fast_hash::FxHashMap;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn host_dispatch(ptr: u32, len: u32) -> u64;
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_dispatch(_ptr: u32, _len: u32) -> u64 {
    panic!(
        "mod-sdk host call made off-wasm with no host installed on this thread \
         (see mod_sdk::testing::MockHost / install_host)"
    )
}

pub struct ModSlot<T>(UnsafeCell<Option<T>>);

unsafe impl<T> Sync for ModSlot<T> {}

impl<T> ModSlot<T> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }
}

pub fn alloc(len: u32) -> u32 {
    if len == 0 {
        return 4;
    }
    let layout = core::alloc::Layout::from_size_align(len as usize, 1).unwrap();
    let ptr = unsafe { std::alloc::alloc(layout) };
    assert!(!ptr.is_null(), "guest allocation of {len} bytes failed");
    ptr as u32
}

pub fn free(ptr: u32, len: u32) {
    if len == 0 {
        return;
    }
    let layout = core::alloc::Layout::from_size_align(len as usize, 1).unwrap();
    unsafe { std::alloc::dealloc(ptr as *mut u8, layout) };
}

fn to_wire(bytes: &[u8]) -> u64 {
    let ptr = alloc(bytes.len() as u32);
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len()) };
    mod_api::pack_ptr_len(ptr, bytes.len() as u32)
}

pub fn host_call(call: &HostCall) -> HostRet {
    match host_call_reply(call) {
        Answer::Native(ret) => ret,
        Answer::Guest(reply) => reply.decode(),
    }
}

pub enum Answer {
    Native(HostRet),
    Guest(Reply),
}

pub enum Reply {
    /// A reply the host wrote into guest memory; freed when this drops.
    Guest { ptr: u32, len: u32 },
    /// A reply a native test host answered.
    Owned(Vec<u8>),
}

impl Reply {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Reply::Guest { ptr, len } => unsafe {
                core::slice::from_raw_parts(*ptr as *const u8, *len as usize)
            },
            Reply::Owned(bytes) => bytes,
        }
    }

    pub fn decode(self) -> HostRet {
        mod_api::decode(self.bytes()).expect("malformed host reply")
    }

    /// The reply through one variant's decoder ([`mod_api::ret_decode`]): only that variant's
    /// payload type is linked into the guest.
    pub fn decode_as<T>(
        self,
        decoder: fn(&[u8]) -> Result<T, mod_api::ReplyMismatch>,
    ) -> Result<T, mod_api::ReplyMismatch> {
        decoder(self.bytes())
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Reply::Guest { ptr, len } = *self {
            free(ptr, len);
        }
    }
}

/// One host call sent as `[domain][call][fields]` from the domain enum value itself, so a guest
/// links the serializer of the domains it uses instead of the whole [`HostCall`].
pub fn call<D: mod_api::WireDomain + Clone + Into<HostCall>>(call: &D) -> Reply {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ret) = crate::testing::answer_natively(&call.clone().into()) {
        return Reply::Owned(mod_api::encode(&ret).expect("encode host reply"));
    }
    let packed = WIRE.with(|buf| match buf.try_borrow_mut() {
        Ok(mut buf) => {
            let len = mod_api::encode_call_into(call, &mut buf);
            unsafe { host_dispatch(buf.as_ptr() as u32, len as u32) }
        }
        // A host call made while this buffer is borrowed (from inside an encode) gets its own.
        Err(_) => {
            let mut own = Vec::new();
            let len = mod_api::encode_call_into(call, &mut own);
            unsafe { host_dispatch(own.as_ptr() as u32, len as u32) }
        }
    });
    let (ptr, len) = mod_api::unpack_ptr_len(packed);
    Reply::Guest { ptr, len }
}

/// A typed reply, or the panic a wrong one is (which traps and disables the mod).
pub fn expect_value<T>(what: &str, reply: Result<T, mod_api::ReplyMismatch>) -> T {
    match reply {
        Ok(value) => value,
        Err(mod_api::ReplyMismatch::Err(e)) => panic!("{what} rejected: {e}"),
        Err(mod_api::ReplyMismatch::Unsupported) => panic!("{what} is not supported by this host"),
        Err(mod_api::ReplyMismatch::Other(index)) => {
            panic!("{what} returned reply variant #{index}")
        }
        Err(mod_api::ReplyMismatch::Malformed) => panic!("{what} returned a malformed reply"),
    }
}

/// A typed reply, a recoverable refusal as `Err`, anything else the panic it is.
pub fn recoverable_value<T>(
    what: &str,
    reply: Result<T, mod_api::ReplyMismatch>,
) -> Result<T, mod_api::HostError> {
    match reply {
        Err(mod_api::ReplyMismatch::Err(e)) if e.code.is_recoverable() => Err(e),
        other => Ok(expect_value(what, other)),
    }
}

pub fn host_call_reply(call: &HostCall) -> Answer {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ret) = crate::testing::answer_natively(call) {
        return Answer::Native(ret);
    }
    let packed = WIRE.with(|buf| {
        // A host call made while this buffer is borrowed (from inside an encode) gets its own.
        let Ok(mut buf) = buf.try_borrow_mut() else {
            let request = mod_api::encode(call).expect("encode host call");
            return unsafe { host_dispatch(request.as_ptr() as u32, request.len() as u32) };
        };
        let len = mod_api::encode_into(call, &mut buf).expect("encode host call");
        unsafe { host_dispatch(buf.as_ptr() as u32, len as u32) }
    });
    let (ptr, len) = mod_api::unpack_ptr_len(packed);
    Answer::Guest(Reply::Guest { ptr, len })
}

pub fn expect_unit(what: &str, ret: HostRet) {
    match ret {
        HostRet::Unit => {}
        HostRet::Err(e) => panic!("{what} rejected: {e}"),
        other => panic!("{what} returned {other:?}"),
    }
}

/// Public host-call wrapper. Builds the `HostCall`, sends it, and decodes the only reply shape
/// the call is allowed to get. Anything else panics, which traps and disables the mod.
///
/// No `->` means the call replies `Unit` (registrations, actions). `=> Variant` hands back that
/// variant's payload as-is, and `=> pattern => expr` is for a one-off reply you map by hand.
/// Conversions like `key: key.into()` go right in the struct-literal body.
macro_rules! host_fn {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $aty:ty),* $(,)?)
            => $call:ident $({ $($field:tt)* })?
    ) => {
        $(#[$meta])*
        $vis fn $name($($arg: $aty),*) {
            $crate::__rt::expect_value(
                stringify!($call),
                $crate::__rt::call(&$crate::calls::$call $({ $($field)* })?)
                    .decode_as($crate::ret_decode::Unit),
            );
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $aty:ty),* $(,)?) -> $ret:ty
            => $call:ident $({ $($field:tt)* })?
            => $retvar:ident
    ) => {
        $(#[$meta])*
        $vis fn $name($($arg: $aty),*) -> $ret {
            $crate::__rt::expect_value(
                stringify!($call),
                $crate::__rt::call(&$crate::calls::$call $({ $($field)* })?)
                    .decode_as($crate::ret_decode::$retvar),
            )
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $aty:ty),* $(,)?) -> $ret:ty
            => $call:ident $({ $($field:tt)* })?
            => $retpat:pat => $map:expr
    ) => {
        $(#[$meta])*
        $vis fn $name($($arg: $aty),*) -> $ret {
            match $crate::__rt::host_call(&$crate::HostCall::from($crate::calls::$call $({ $($field)* })?)) {
                $retpat => $map,
                other => panic!("{} returned {other:?}", stringify!($call)),
            }
        }
    };
}
pub(crate) use host_fn;

pub fn recoverable(what: &str, ret: HostRet) -> Result<HostRet, mod_api::HostError> {
    match ret {
        HostRet::Err(error) if error.code.is_recoverable() => Err(error),
        HostRet::Err(error) => panic!("{what} rejected: {error}"),
        other => Ok(other),
    }
}

macro_rules! try_host_fn {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $aty:ty),* $(,)?)
            => $call:ident $({ $($field:tt)* })?
    ) => {
        $(#[$meta])*
        $vis fn $name($($arg: $aty),*) -> Result<(), $crate::HostError> {
            $crate::__rt::recoverable_value(
                stringify!($call),
                $crate::__rt::call(&$crate::calls::$call $({ $($field)* })?)
                    .decode_as($crate::ret_decode::Unit),
            )
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $aty:ty),* $(,)?) -> $ret:ty
            => $call:ident $({ $($field:tt)* })?
            => $retvar:ident
    ) => {
        $(#[$meta])*
        $vis fn $name($($arg: $aty),*) -> Result<$ret, $crate::HostError> {
            $crate::__rt::recoverable_value(
                stringify!($call),
                $crate::__rt::call(&$crate::calls::$call $({ $($field)* })?)
                    .decode_as($crate::ret_decode::$retvar),
            )
        }
    };
}
pub(crate) use try_host_fn;

pub fn init<T: crate::Mod>(slot: &ModSlot<T>, host_abi: u32, host_caps: u64) {
    crate::abi::record_host(host_abi, host_caps);
    std::panic::set_hook(Box::new(|info| {
        let _ = call(&mod_api::calls::Log {
            msg: format!("PANIC: {info}"),
        });
    }));
    let slot = unsafe { &mut *slot.0.get() };
    slot.insert(T::default()).init();
}

pub fn dispatch<T: crate::Mod>(slot: &ModSlot<T>, ptr: u32, len: u32) -> u64 {
    let decoded = {
        let request = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        let decoded = mod_api::decode_call::<GuestCall>(request).expect("malformed engine call");
        free(ptr, len);
        decoded
    };
    let mod_ = unsafe { (*slot.0.get()).as_mut() }.expect("mod_dispatch before mod_init");
    let ret = match decoded {
        Decoded::Known(call) => dispatch_call(mod_, call),
        Decoded::Unknown { .. } => GuestRet::Unsupported,
    };
    WIRE.with(|buf| {
        let mut buf = buf.borrow_mut();
        let len = mod_api::encode_into(&ret, &mut buf).expect("encode guest reply");
        to_wire(&buf[..len])
    })
}

type TagMap = FxHashMap<u64, Vec<(String, mod_api::MobTagValue)>>;

thread_local! {
    /// Where replies and host-call requests are serialized: kept between calls, so a message
    /// grows no buffer once one of its size has been sent.
    static WIRE: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Per AI node, the tags of the mobs of its latest batch (see `GuestCall::AiNodeBatch`).
    static AI_TAGS: std::cell::RefCell<FxHashMap<u32, TagMap>> =
        std::cell::RefCell::new(FxHashMap::default());
}

fn dispatch_call<T: crate::Mod>(mod_: &mut T, call: GuestCall) -> GuestRet {
    match call {
        GuestCall::TickSystem { id } => {
            mod_.tick_system(id);
            GuestRet::Unit
        }
        GuestCall::HandleEvent { id, mut payload } => {
            let outcome = mod_.handle_event(id, &mut payload);
            let payload = payload.kind().echoes_payload().then_some(payload);
            GuestRet::Event { outcome, payload }
        }
        GuestCall::GenFeature {
            feature_id,
            section_pos,
            seed,
            blocks,
            surface_heights,
            biomes,
            sea_level,
        } => {
            let ctx = crate::GenCtx::from_wire(
                section_pos,
                seed,
                blocks,
                surface_heights,
                biomes,
                sea_level,
            );
            GuestRet::GenOutput(mod_.gen_feature(feature_id, &ctx))
        }
        GuestCall::GenClaims {
            feature_id,
            seed,
            sea_level,
            min,
            max,
        } => GuestRet::GenClaims(mod_.gen_claims(
            feature_id,
            &crate::ClaimsCtx {
                seed,
                sea_level,
                min,
                max,
            },
        )),
        GuestCall::GenStage {
            callback_id,
            stage,
            section_pos,
            seed,
            blocks,
            surface_heights,
            biomes,
            sea_level,
        } => {
            let ctx = crate::GenCtx::from_wire(
                section_pos,
                seed,
                blocks,
                surface_heights,
                biomes,
                sea_level,
            );
            match stage {
                mod_api::WorldgenStage::Climate => {
                    GuestRet::GenBiomes(mod_.gen_climate(callback_id, &ctx))
                }
                mod_api::WorldgenStage::Terrain => {
                    GuestRet::GenBlocks(mod_.gen_terrain(callback_id, &ctx))
                }
                other => GuestRet::GenOutput(mod_.gen_stage(callback_id, other, &ctx)),
            }
        }
        GuestCall::GuiClick {
            kind_key,
            widget_id,
            at,
        } => {
            mod_.gui_click(&kind_key, &widget_id, at);
            GuestRet::Unit
        }
        GuestCall::HostileSpawnCandidate {
            callback_id,
            candidate,
        } => GuestRet::HostileSpawn(mod_.hostile_spawn_candidate(callback_id, &candidate)),
        GuestCall::BlockBehavior {
            callback_id,
            kind,
            pos,
        } => {
            mod_.block_hook(callback_id, kind, pos);
            GuestRet::Unit
        }
        GuestCall::AiNode { callback_id, ctx } => {
            GuestRet::AiDecision(mod_.ai_node(callback_id, &ctx))
        }
        GuestCall::AiNodeBatch {
            callback_id,
            mut ctxs,
            tags,
        } => AI_TAGS.with(|cache| {
            // The host sends a mob's tags only when they changed since the last batch this
            // instance saw the mob in; the mobs of the latest batch are all either side keeps.
            let mut cache = cache.borrow_mut();
            let mut last = cache.remove(&callback_id).unwrap_or_default();
            let mut next = FxHashMap::default();
            let mut sent = tags.into_iter();
            let decisions = ctxs
                .iter_mut()
                .map(|ctx| {
                    ctx.tags = match sent.next().flatten() {
                        Some(tags) => tags,
                        None => last.remove(&ctx.mob_id).unwrap_or_default(),
                    };
                    let decision = mod_.ai_node(callback_id, ctx);
                    next.insert(ctx.mob_id, core::mem::take(&mut ctx.tags));
                    decision
                })
                .collect();
            cache.insert(callback_id, next);
            GuestRet::AiDecisions(decisions)
        }),
        GuestCall::ClientFrame { frame } => {
            mod_.client_frame(&frame);
            GuestRet::Unit
        }
        GuestCall::ClientKey { action_id, pressed } => {
            mod_.client_key(action_id, pressed);
            GuestRet::Unit
        }
        GuestCall::ClientUi { kind_key, event } => {
            mod_.client_ui(&kind_key, &event);
            GuestRet::Unit
        }
        GuestCall::ClientCanvas { canvas_key, event } => {
            mod_.client_canvas(&canvas_key, &event);
            GuestRet::Unit
        }
        GuestCall::ClientCanvasScroll {
            canvas_key,
            x,
            y,
            delta,
        } => {
            mod_.client_canvas_scroll(&canvas_key, x, y, delta);
            GuestRet::Unit
        }
        GuestCall::BakeShapeSim { shape_kind, cells } => {
            GuestRet::BakedSim(mod_.bake_shape_sim(shape_kind, &cells))
        }
        GuestCall::BakeShapeRender { shape_kind, cells } => {
            GuestRet::BakedRender(mod_.bake_shape_render(shape_kind, &cells))
        }
        GuestCall::BakeShapeItem {
            shape_kind,
            block_id,
        } => GuestRet::BakedItem(mod_.bake_shape_item(shape_kind, block_id)),
        GuestCall::ShapePlacementPlan {
            shape_kind,
            block_id,
            inputs,
        } => GuestRet::ShapePlacement(mod_.shape_placement_plan(shape_kind, block_id, &inputs)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::{AiNodeCtx, AiNodeDecision, MobTagValue, PlayerId};

    #[derive(Default)]
    struct Seen {
        tags: Vec<(u64, Vec<(String, MobTagValue)>)>,
    }

    impl crate::Mod for Seen {
        fn init(&mut self) {}
        fn ai_node(&mut self, _: u32, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
            self.tags.push((ctx.mob_id, ctx.tags.clone()));
            None
        }
    }

    fn ctx(mob_id: u64) -> AiNodeCtx {
        AiNodeCtx {
            mob_id,
            pos: [0.0; 3],
            cell: [0; 3],
            yaw: 0.0,
            tick: 1,
            player_id: PlayerId(1),
            player_pos: [0.0; 3],
            nav_idle: true,
            in_fluid: None,
            target: None,
            attacker: None,
            player_held: None,
            player_foothold: None,
            tags: Vec::new(),
        }
    }

    fn tag(k: &str, v: i64) -> (String, MobTagValue) {
        (k.to_owned(), MobTagValue::I64(v))
    }

    #[test]
    fn a_batch_reuses_unchanged_tags_and_forgets_absent_mobs() {
        let mut seen = Seen::default();
        let batch = |ctxs: Vec<u64>, tags: Vec<Option<Vec<(String, MobTagValue)>>>| {
            GuestCall::AiNodeBatch {
                callback_id: 3,
                ctxs: ctxs.into_iter().map(ctx).collect(),
                tags,
            }
        };
        dispatch_call(
            &mut seen,
            batch(
                vec![1, 2],
                vec![Some(vec![tag("a", 1)]), Some(vec![tag("b", 2)])],
            ),
        );
        dispatch_call(
            &mut seen,
            batch(vec![1, 2], vec![None, Some(vec![tag("b", 3)])]),
        );
        // Mob 2 drops out; mob 1 keeps riding on its last tags; mob 3 arrives whole.
        dispatch_call(
            &mut seen,
            batch(vec![1, 3], vec![None, Some(vec![tag("c", 4)])]),
        );
        // Mob 2 comes back whole (the host resends after an absence); mob 3 unchanged.
        dispatch_call(
            &mut seen,
            batch(vec![2, 3], vec![Some(vec![tag("b", 5)]), None]),
        );
        let got: Vec<(u64, Vec<(String, MobTagValue)>)> = seen.tags.clone();
        assert_eq!(
            got,
            vec![
                (1, vec![tag("a", 1)]),
                (2, vec![tag("b", 2)]),
                (1, vec![tag("a", 1)]),
                (2, vec![tag("b", 3)]),
                (1, vec![tag("a", 1)]),
                (3, vec![tag("c", 4)]),
                (2, vec![tag("b", 5)]),
                (3, vec![tag("c", 4)]),
            ]
        );
        // A different node id has its own cache.
        dispatch_call(
            &mut seen,
            GuestCall::AiNodeBatch {
                callback_id: 4,
                ctxs: vec![ctx(3)],
                tags: vec![None],
            },
        );
        assert_eq!(
            seen.tags.last().unwrap().1,
            Vec::new(),
            "no tags were ever sent for node 4"
        );
    }
}
