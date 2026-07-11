//! ABI plumbing for [`register_mod!`](crate::register_mod). Not mod-facing
//! API — everything here is `#[doc(hidden)]` and may change with the SDK.

use core::cell::UnsafeCell;

use mod_api::{GuestCall, GuestRet, HostCall, HostRet};

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn host_dispatch(ptr: u32, len: u32) -> u64;
}

/// Host-target stub so the SDK itself type-checks off-wasm (mods only ever
/// build for wasm32; the engine never links this crate).
#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_dispatch(_ptr: u32, _len: u32) -> u64 {
    unreachable!("mod-sdk host calls only exist inside the wasm guest")
}

/// The single mod instance behind the raw exports.
///
/// SAFETY: the wasm guest is single-threaded by construction (the host
/// disables wasm threads), so unsynchronized interior mutability is sound
/// there; the `Sync` impl exists only to allow the `static`.
pub struct ModSlot<T>(UnsafeCell<Option<T>>);

unsafe impl<T> Sync for ModSlot<T> {}

impl<T> ModSlot<T> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }
}

/// Guest-side buffers cross the ABI as raw byte allocations with align 1;
/// `alloc`/`free` are the exported allocator the HOST also uses to hand
/// buffers in (requests) and reclaim buffers it read (replies).
pub fn alloc(len: u32) -> u32 {
    if len == 0 {
        return 4; // non-null, never dereferenced nor freed (len 0)
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

/// Encode a byte buffer into a fresh allocation and pack its address for
/// the `u64` return lane. The receiver frees it via [`free`].
fn to_wire(bytes: &[u8]) -> u64 {
    let ptr = alloc(bytes.len() as u32);
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len()) };
    mod_api::pack_ptr_len(ptr, bytes.len() as u32)
}

/// One host call: encode, dispatch, decode the reply (the host allocated
/// it in our memory through `mod_alloc`; we own and free it).
pub fn host_call(call: &HostCall) -> HostRet {
    let request = mod_api::encode(call).expect("encode host call");
    let packed = unsafe { host_dispatch(request.as_ptr() as u32, request.len() as u32) };
    let (ptr, len) = mod_api::unpack_ptr_len(packed);
    let reply = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let ret = mod_api::decode(reply).expect("malformed host reply");
    free(ptr, len);
    ret
}

/// Registration replies must be `Unit`; an [`HostRet::Error`] (e.g.
/// registering outside `mod_init`) is a mod bug — panic loudly, which
/// traps and disables the mod.
pub fn expect_unit(what: &str, ret: HostRet) {
    match ret {
        HostRet::Unit => {}
        HostRet::Error(e) => panic!("{what} rejected: {e}"),
        other => panic!("{what} returned {other:?}"),
    }
}

pub fn init<T: crate::Mod>(slot: &ModSlot<T>) {
    // Panics abort the guest (a trap); surface the message through the
    // host log first so the disable line has a cause next to it.
    std::panic::set_hook(Box::new(|info| {
        let _ = host_call(&HostCall::Log {
            msg: format!("PANIC: {info}"),
        });
    }));
    let slot = unsafe { &mut *slot.0.get() };
    slot.insert(T::default()).init();
}

pub fn dispatch<T: crate::Mod>(slot: &ModSlot<T>, ptr: u32, len: u32) -> u64 {
    let call: GuestCall = {
        let request = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        let call = mod_api::decode(request).expect("malformed engine call");
        free(ptr, len); // the guest owns request buffers once dispatched
        call
    };
    let mod_ = unsafe { (*slot.0.get()).as_mut() }.expect("mod_dispatch before mod_init");
    let ret = match call {
        GuestCall::TickSystem { id } => {
            mod_.tick_system(id);
            GuestRet::Unit
        }
        GuestCall::HandleEvent {
            id,
            kind: _,
            mut payload,
        } => {
            let outcome = mod_.handle_event(id, &mut payload);
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
            GuestRet::GenWrites(mod_.gen_feature(feature_id, &ctx))
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
                other => GuestRet::GenWrites(mod_.gen_stage(callback_id, other, &ctx)),
            }
        }
        GuestCall::GuiClick {
            kind_key,
            widget_id,
            pos,
        } => {
            mod_.gui_click(&kind_key, &widget_id, pos);
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
    };
    to_wire(&mod_api::encode(&ret).expect("encode guest reply"))
}
