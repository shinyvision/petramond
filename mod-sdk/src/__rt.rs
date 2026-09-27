use core::cell::UnsafeCell;

use mod_api::{Decoded, GuestCall, GuestRet, HostCall, HostRet};

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

pub struct Reply {
    ptr: u32,
    len: u32,
}

impl Reply {
    pub fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len as usize) }
    }

    pub fn decode(self) -> HostRet {
        mod_api::decode(self.bytes()).expect("malformed host reply")
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        free(self.ptr, self.len);
    }
}

pub fn host_call_reply(call: &HostCall) -> Answer {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ret) = crate::testing::answer_natively(call) {
        return Answer::Native(ret);
    }
    let request = mod_api::encode(call).expect("encode host call");
    let packed = unsafe { host_dispatch(request.as_ptr() as u32, request.len() as u32) };
    let (ptr, len) = mod_api::unpack_ptr_len(packed);
    Answer::Guest(Reply { ptr, len })
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
            $crate::__rt::expect_unit(
                stringify!($call),
                $crate::__rt::host_call(&$crate::HostCall::from($crate::calls::$call $({ $($field)* })?)),
            );
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $aty:ty),* $(,)?) -> $ret:ty
            => $call:ident $({ $($field:tt)* })?
            => $retvar:ident
    ) => {
        $crate::__rt::host_fn! {
            $(#[$meta])*
            $vis fn $name($($arg: $aty),*) -> $ret
                => $call $({ $($field)* })?
                => $crate::HostRet::$retvar(__value) => __value
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
            let ret = $crate::__rt::recoverable(
                stringify!($call),
                $crate::__rt::host_call(&$crate::HostCall::from($crate::calls::$call $({ $($field)* })?)),
            )?;
            match ret {
                $crate::HostRet::Unit => Ok(()),
                other => panic!("{} returned {other:?}", stringify!($call)),
            }
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
            let ret = $crate::__rt::recoverable(
                stringify!($call),
                $crate::__rt::host_call(&$crate::HostCall::from($crate::calls::$call $({ $($field)* })?)),
            )?;
            match ret {
                $crate::HostRet::$retvar(value) => Ok(value),
                other => panic!("{} returned {other:?}", stringify!($call)),
            }
        }
    };
}
pub(crate) use try_host_fn;

pub fn init<T: crate::Mod>(slot: &ModSlot<T>, host_abi: u32, host_caps: u64) {
    crate::abi::record_host(host_abi, host_caps);
    std::panic::set_hook(Box::new(|info| {
        let _ = host_call(&HostCall::from(mod_api::calls::Log {
            msg: format!("PANIC: {info}"),
        }));
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
    to_wire(&mod_api::encode(&ret).expect("encode guest reply"))
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
            let ctx = crate::GenCtx {
                section_pos,
                seed,
                blocks,
                surface_heights,
                biomes,
                sea_level,
            };
            GuestRet::GenOutput(mod_.gen_feature(feature_id, &ctx))
        }
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
            let ctx = crate::GenCtx {
                section_pos,
                seed,
                blocks,
                surface_heights,
                biomes,
                sea_level,
            };
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
        GuestCall::AiNodeBatch { callback_id, ctxs } => GuestRet::AiDecisions(
            ctxs.iter()
                .map(|ctx| mod_.ai_node(callback_id, ctx))
                .collect(),
        ),
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
