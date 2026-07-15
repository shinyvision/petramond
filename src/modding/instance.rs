//! One loaded mod: its wasmtime store + instance, the raw dispatch protocol,
//! and the disable-on-error policy.
//!
//! Protocol (see `mod-api` docs): requests are postcard bytes written into
//! guest memory through the guest's own `mod_alloc`; `mod_dispatch(ptr, len)`
//! consumes the request buffer and returns a packed `ptr << 32 | len` reply
//! the host reads and then releases with `mod_free`. Any trap, deadline,
//! memory fault, or malformed reply DISABLES the mod for the session with a
//! visible error — the tick always continues without it.

use mod_api::{GuestCall, GuestRet};
use wasmtime::{Memory, Module, Store, TypedFunc};

use crate::events::SimCtx;

use super::host::{self, ModStoreData, Phase, Registration, DISPATCH_DEADLINE_EPOCHS};
use super::scope;

pub(super) struct ModInstance {
    id: String,
    store: Store<ModStoreData>,
    memory: Memory,
    fn_init: TypedFunc<(), ()>,
    fn_alloc: TypedFunc<u32, u32>,
    fn_free: TypedFunc<(u32, u32), ()>,
    fn_dispatch: TypedFunc<(u32, u32), u64>,
    disabled: bool,
    /// Successful guest dispatches (init + tick systems + events), for tests
    /// and diagnostics.
    dispatches: u64,
}

impl ModInstance {
    pub(super) fn from_module(id: &str, module: &Module, world_seed: u32) -> Result<Self, String> {
        Self::from_module_side(id, module, world_seed, mod_api::RuntimeSide::Server, None)
    }

    pub(super) fn from_module_side(
        id: &str,
        module: &Module,
        world_seed: u32,
        side: mod_api::RuntimeSide,
        client_storage_dir: Option<std::path::PathBuf>,
    ) -> Result<Self, String> {
        let mut store = Store::new(
            host::engine(),
            ModStoreData::new_for_side(id, world_seed, side, client_storage_dir),
        );
        store.limiter(|data| &mut data.limits);
        // Instantiation runs guest code too (data/start sections): same leash.
        store.set_epoch_deadline(DISPATCH_DEADLINE_EPOCHS);
        let instance = host::linker()?
            .instantiate(&mut store, module)
            .map_err(|e| format!("instantiate: {e:#}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("mod exports no linear memory")?;
        let typed_err = |name: &str, e: wasmtime::Error| format!("export {name}: {e:#}");
        let fn_init = instance
            .get_typed_func::<(), ()>(&mut store, "mod_init")
            .map_err(|e| typed_err("mod_init", e))?;
        let fn_alloc = instance
            .get_typed_func::<u32, u32>(&mut store, "mod_alloc")
            .map_err(|e| typed_err("mod_alloc", e))?;
        let fn_free = instance
            .get_typed_func::<(u32, u32), ()>(&mut store, "mod_free")
            .map_err(|e| typed_err("mod_free", e))?;
        let fn_dispatch = instance
            .get_typed_func::<(u32, u32), u64>(&mut store, "mod_dispatch")
            .map_err(|e| typed_err("mod_dispatch", e))?;
        store.data_mut().memory = Some(memory);
        store.data_mut().alloc = Some(fn_alloc.clone());
        Ok(Self {
            id: id.to_owned(),
            store,
            memory,
            fn_init,
            fn_alloc,
            fn_free,
            fn_dispatch,
            disabled: false,
            dispatches: 0,
        })
    }

    pub(super) fn disabled(&self) -> bool {
        self.disabled
    }

    #[cfg_attr(not(test), allow(dead_code))] // test observability
    pub(super) fn dispatches(&self) -> u64 {
        self.dispatches
    }

    #[cfg_attr(not(test), allow(dead_code))] // test observability
    pub(super) fn stats(&self) -> super::host::HostStats {
        self.store.data().stats
    }

    /// Run `mod_init` — the mod's one registration window. On return the
    /// window closes; a trapped init disables the mod and DROPS its partial
    /// registrations (a mod is never half-loaded).
    pub(super) fn call_init(&mut self, ctx: &mut SimCtx) {
        scope::enter(ctx, || self.call_init_detached());
    }

    /// [`call_init`](Self::call_init) WITHOUT publishing a simulation context —
    /// how per-thread worldgen instances initialize: registrations are still
    /// accepted (and later ignored — the MAIN load already recorded them), but
    /// any sim-scoped host call gets `HostRet::Error`, so a gen-hook mod's init
    /// must stay pure (registrations, `ResolveBlock`, `Log`, `RngU64`).
    pub(super) fn call_init_detached(&mut self) {
        debug_assert!(self.store.data().phase == Phase::Init);
        self.arm_dispatch();
        let result = self.fn_init.call(&mut self.store, ());
        self.store.data_mut().phase = Phase::Run;
        match result {
            Ok(()) => self.dispatches += 1,
            Err(e) => {
                let context = self.dispatch_context(None);
                self.disable(&format!("mod_init trapped: {e:#}{context}"));
            }
        }
    }

    /// The registrations `mod_init` collected — empty if the mod got disabled.
    pub(super) fn take_registrations(&mut self) -> Vec<Registration> {
        if self.disabled {
            self.store.data_mut().pending.clear();
            return Vec::new();
        }
        std::mem::take(&mut self.store.data_mut().pending)
    }

    /// Dispatch one [`GuestCall`], publishing `ctx` for re-entrant host calls.
    /// `None` = the mod is (or just became) disabled; the caller carries on.
    pub(super) fn call_guest(&mut self, ctx: &mut SimCtx, call: &GuestCall) -> Option<GuestRet> {
        scope::enter(ctx, || self.call_guest_detached(call))
    }

    /// [`call_guest`](Self::call_guest) WITHOUT publishing a simulation
    /// context — the worldgen dispatch path (worker threads and any thread
    /// running `generate_*`): sim-scoped host calls made during the dispatch
    /// are rejected, everything else (deadline, disable-on-trap, protocol)
    /// behaves identically.
    pub(super) fn call_guest_detached(&mut self, call: &GuestCall) -> Option<GuestRet> {
        if self.disabled {
            return None;
        }
        let request = match mod_api::encode(call) {
            Ok(bytes) => bytes,
            Err(e) => {
                // Host-side bug, but never let it poison the sim either.
                self.disable(&format!("encode guest call: {e}"));
                return None;
            }
        };
        self.arm_dispatch();
        let started = std::time::Instant::now();
        match self.dispatch_protocol(&request) {
            Ok(ret) => {
                self.dispatches += 1;
                self.log_slow_dispatch(call, started.elapsed());
                Some(ret)
            }
            Err(e) => {
                let context = self.dispatch_context(Some(call));
                self.disable(&format!("{e}{context}"));
                None
            }
        }
    }

    /// Perf diagnostics: any dispatch over the threshold logs its guest/host
    /// wall split under the `petramond::modding::perf` target, so frame
    /// stutter attributes to the mod, the call, and the side of the ABI it
    /// spent its time on.
    fn log_slow_dispatch(&self, call: &GuestCall, total: std::time::Duration) {
        const SLOW_DISPATCH: std::time::Duration = std::time::Duration::from_millis(2);
        if total < SLOW_DISPATCH
            || !log::log_enabled!(target: "petramond::modding::perf", log::Level::Debug)
        {
            return;
        }
        let data = self.store.data();
        let host = data.dispatch_host_wall;
        log::debug!(
            target: "petramond::modding::perf",
            "slow mod dispatch '{}': {} took {:.1?} (guest {:.1?}, host {:.1?} across {} host calls)",
            self.id,
            host::short_debug(call, 48),
            total,
            total.saturating_sub(host),
            host,
            data.dispatch_host_calls(),
        );
    }

    /// Arm the watchdog for one guest entry: the store's epoch deadline plus
    /// the per-dispatch accounting `host_dispatch` charges against.
    fn arm_dispatch(&mut self) {
        self.store.set_epoch_deadline(DISPATCH_DEADLINE_EPOCHS);
        self.store.data_mut().begin_dispatch();
    }

    /// Diagnostic suffix for disable messages: the guest call that was in
    /// flight and the dispatch's most recent host call — a failure names what
    /// was actually happening instead of just the trap kind.
    fn dispatch_context(&self, call: Option<&GuestCall>) -> String {
        let mut out = String::new();
        if let Some(call) = call {
            out.push_str(&format!(
                " [guest call: {}]",
                host::short_debug(call, host::DIAG_DEBUG_CAP)
            ));
        }
        match &self.store.data().last_host_call {
            Some((desc, true)) => out.push_str(&format!(" [last host call (returned): {desc}]")),
            Some((desc, false)) => out.push_str(&format!(" [host call in flight: {desc}]")),
            None => {}
        }
        out
    }

    pub(super) fn call_guest_client(
        &mut self,
        world: &crate::world::World,
        call: &GuestCall,
    ) -> Option<GuestRet> {
        super::client::scope::enter(world, || self.call_guest_detached(call))
    }

    pub(super) fn client_data(&self) -> Option<&super::client::ClientStoreData> {
        self.store.data().client.as_ref()
    }

    pub(super) fn client_data_mut(&mut self) -> Option<&mut super::client::ClientStoreData> {
        self.store.data_mut().client.as_mut()
    }

    /// The raw request/reply protocol of one dispatch (see the module docs).
    fn dispatch_protocol(&mut self, request: &[u8]) -> Result<GuestRet, String> {
        let len = request.len() as u32;
        let ptr = self
            .fn_alloc
            .call(&mut self.store, len)
            .map_err(|e| format!("mod_alloc: {e:#}"))?;
        self.memory
            .write(&mut self.store, ptr as usize, request)
            .map_err(|e| format!("write request: {e:#}"))?;
        let packed = self
            .fn_dispatch
            .call(&mut self.store, (ptr, len))
            .map_err(|e| format!("mod_dispatch: {e:#}"))?;
        let (reply_ptr, reply_len) = mod_api::unpack_ptr_len(packed);
        // Bounds-check BEFORE sizing the copy so a hostile reply length
        // can't balloon a host allocation.
        if reply_len as usize > self.memory.data_size(&self.store) {
            return Err("reply exceeds guest memory".to_owned());
        }
        let mut reply = vec![0u8; reply_len as usize];
        self.memory
            .read(&self.store, reply_ptr as usize, &mut reply)
            .map_err(|e| format!("read reply: {e:#}"))?;
        self.fn_free
            .call(&mut self.store, (reply_ptr, reply_len))
            .map_err(|e| format!("mod_free: {e:#}"))?;
        mod_api::decode(&reply).map_err(|e| format!("malformed guest reply: {e}"))
    }

    /// Session-scoped kill switch: one visible error line, then the mod stops
    /// receiving dispatches until the next launch.
    pub(super) fn disable(&mut self, why: &str) {
        if self.disabled {
            return;
        }
        self.disabled = true;
        log::error!("mod '{}' disabled for this session: {why}", self.id);
    }
}
