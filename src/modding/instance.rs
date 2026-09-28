//! One loaded mod: its wasmtime store + instance, the raw dispatch protocol,
//! and the disable-on-error policy.
//!
//! Handshake: right after instantiation — before `mod_init` — the host reads
//! the guest's `mod_abi_version`/`mod_abi_requires` exports and refuses a
//! module whose ABI major or required capabilities it cannot serve
//! ([`mod_api::negotiate`]); an accepted guest gets the host's version and
//! capability bits as `mod_init(abi, caps)`.
//!
//! Protocol (see `mod-api` docs): requests are postcard bytes written into
//! guest memory through the guest's own `mod_alloc`; `mod_dispatch(ptr, len)`
//! consumes the request buffer and returns a packed `ptr << 32 | len` reply
//! the host reads and then releases with `mod_free`. Any trap, emergency
//! deadline, memory fault, or malformed reply DISABLES the mod for the
//! session with a visible error — the tick always continues without it — and
//! the disable lands on the mod's shared [`ModHealth`], so every other
//! instance of it (worldgen workers, bakes) stops too. A
//! [`GuestRet::Unsupported`] reply (an older guest declining a call it
//! predates) is not an error: the dispatch counts as unanswered and the mod
//! stays enabled.

use std::sync::Arc;

use mod_api::{AbiRejection, AbiVersion, Capabilities, GuestCall, GuestRet};
use wasmtime::{Instance, Memory, Module, Store, TypedFunc};

use crate::events::SimCtx;

#[inline]
fn slow_dispatch_logging() -> bool {
    log::log_enabled!(target: "petramond::modding::perf", log::Level::Debug)
}

use super::health::ModHealth;
use super::host::{self, ModStoreData, Phase, Registration};
use super::scope;
use super::watchdog::{throttle_notice, CallClass, Context};

pub(super) struct ModInstance {
    id: String,
    store: Store<ModStoreData>,
    memory: Memory,
    fn_init: TypedFunc<(u32, u64), ()>,
    fn_alloc: TypedFunc<u32, u32>,
    fn_free: TypedFunc<(u32, u32), ()>,
    fn_dispatch: TypedFunc<(u32, u32), u64>,
    health: Arc<ModHealth>,
    armed_fuel: u64,
    last_fuel: u64,
    dispatches: u64,
    request_buf: Vec<u8>,
    reply_buf: Vec<u8>,
    declined: Vec<std::mem::Discriminant<GuestCall>>,
}

impl ModInstance {
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn from_module(id: &str, module: &Module, world_seed: u32) -> Result<Self, String> {
        Self::from_module_side(
            id,
            module,
            world_seed,
            mod_api::RuntimeSide::Server,
            None,
            ModHealth::standalone(id),
        )
    }

    pub(super) fn from_module_side(
        id: &str,
        module: &Module,
        world_seed: u32,
        side: mod_api::RuntimeSide,
        client_buckets: Option<super::client::ClientBuckets>,
        health: Arc<ModHealth>,
    ) -> Result<Self, String> {
        let mut store = Store::new(
            host::engine(),
            ModStoreData::new_for_side(id, world_seed, side, client_buckets, Arc::clone(&health)),
        );
        store.limiter(|data| &mut data.limiter);
        store
            .set_fuel(u64::MAX)
            .map_err(|e| format!("arm fuel: {e:#}"))?;
        // A call that reaches its deadline asks the watchdog for more time instead of trapping
        // outright; a refusal traps it.
        store.epoch_deadline_callback(|mut ctx| {
            let left = ctx
                .data_mut()
                .charge_guest_time()
                .map_err(wasmtime::Error::msg)?;
            Ok(wasmtime::UpdateDeadline::Continue(left.max(1)))
        });
        let deadline = store.data_mut().begin_dispatch(Context::Init);
        store.set_epoch_deadline(deadline);
        let instance = host::linker()?
            .instantiate(&mut store, module)
            .map_err(|e| format!("instantiate: {e:#}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("mod exports no linear memory")?;
        let typed_err = |name: &str, e: wasmtime::Error| format!("export {name}: {e:#}");
        let guest_abi = handshake(&instance, &mut store)?;
        log::debug!("mod '{id}' speaks mod ABI {guest_abi}");
        let fn_init = instance
            .get_typed_func::<(u32, u64), ()>(&mut store, "mod_init")
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
        let ty = memory.ty(&store);
        store.data_mut().guest_memory_max = ty.maximum().map_or(host::WASM32_MEMORY_MAX, |pages| {
            pages.saturating_mul(ty.page_size())
        });
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
            request_buf: Vec::new(),
            reply_buf: Vec::new(),
            declined: Vec::new(),
            health,
            armed_fuel: u64::MAX,
            last_fuel: 0,
            dispatches: 0,
        })
    }

    pub(super) fn disabled(&self) -> bool {
        self.health.is_disabled()
    }

    /// Client frames are the beat a client instance is throttled on.
    pub(super) fn begin_client_frame(&mut self) {
        self.store.data_mut().client_period += 1;
    }

    #[cfg(test)]
    pub(super) fn set_budgets_for_test(&mut self, budgets: super::watchdog::Budgets) {
        self.health.watchdog().set_budgets_for_test(budgets);
        self.store.data_mut().throttle = super::watchdog::Throttle::new(&budgets);
    }

    #[cfg(test)]
    pub(super) fn last_dispatch_fuel(&self) -> u64 {
        self.last_fuel
    }

    #[cfg(test)]
    pub(super) fn throttled(&self, context: Context) -> bool {
        self.store.data().throttle.paused(context)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn dispatches(&self) -> u64 {
        self.dispatches
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn stats(&self) -> super::host::HostStats {
        self.store.data().stats
    }

    pub(super) fn call_init(&mut self, ctx: &mut SimCtx) {
        scope::enter(ctx, || self.call_init_detached());
    }

    pub(super) fn call_init_detached(&mut self) {
        debug_assert!(self.store.data().phase == Phase::Init);
        if !self.arm_dispatch(None) {
            self.store.data_mut().phase = Phase::Run;
            return;
        }
        let result = self.fn_init.call(
            &mut self.store,
            (
                mod_api::ABI_VERSION.pack(),
                mod_api::HOST_CAPABILITIES.bits(),
            ),
        );
        self.store.data_mut().phase = Phase::Run;
        self.settle_fuel(None);
        match result {
            Ok(()) => self.dispatches += 1,
            Err(e) => {
                let context = self.dispatch_context(None);
                self.disable(&format!("mod_init trapped: {e:#}{context}"));
            }
        }
    }

    pub(super) fn take_registrations(&mut self) -> Vec<Registration> {
        if self.disabled() {
            self.store.data_mut().pending.clear();
            return Vec::new();
        }
        std::mem::take(&mut self.store.data_mut().pending)
    }

    pub(super) fn call_guest(&mut self, ctx: &mut SimCtx, call: &GuestCall) -> Option<GuestRet> {
        scope::enter(ctx, || self.call_guest_detached(call))
    }

    pub(super) fn call_guest_read_only(
        &mut self,
        ctx: &mut SimCtx,
        call: &GuestCall,
    ) -> Option<GuestRet> {
        scope::enter_read_only(ctx, || self.call_guest_detached(call))
    }

    pub(super) fn call_guest_detached(&mut self, call: &GuestCall) -> Option<GuestRet> {
        self.call_guest_encoded(
            call,
            std::mem::discriminant(call),
            CallClass::of(call),
            call,
        )
    }

    pub(super) fn call_guest_encoded<C: serde::Serialize>(
        &mut self,
        call: &C,
        kind: std::mem::Discriminant<GuestCall>,
        class: CallClass,
        describe: &dyn std::fmt::Debug,
    ) -> Option<GuestRet> {
        if self.disabled() {
            return None;
        }
        let mut request = std::mem::take(&mut self.request_buf);
        let request_len = match mod_api::encode_into(call, &mut request) {
            Ok(len) => len,
            Err(e) => {
                self.request_buf = request;
                self.disable(&format!("encode guest call: {e}"));
                return None;
            }
        };
        if !self.arm_dispatch(Some(class)) {
            self.request_buf = request;
            return None;
        }
        let started = slow_dispatch_logging().then(std::time::Instant::now);
        let result = self.dispatch_protocol(&request[..request_len]);
        self.request_buf = request;
        self.settle_fuel(Some(describe));
        match result {
            Ok(GuestRet::Unsupported) => {
                self.note_declined(kind, describe);
                None
            }
            Ok(ret) => {
                self.dispatches += 1;
                if let Some(started) = started {
                    self.log_slow_dispatch(describe, started.elapsed());
                }
                Some(ret)
            }
            Err(e) => {
                let context = self.dispatch_context(Some(describe));
                self.disable(&format!("{e}{context}"));
                None
            }
        }
    }

    pub(super) fn declines(&self, kind: std::mem::Discriminant<GuestCall>) -> bool {
        self.declined.contains(&kind)
    }

    fn note_declined(
        &mut self,
        kind: std::mem::Discriminant<GuestCall>,
        describe: &dyn std::fmt::Debug,
    ) {
        if self.declined.contains(&kind) {
            return;
        }
        self.declined.push(kind);
        log::warn!(
            "mod '{}' does not support {} (built against an older mod ABI than {}); \
             the engine carries on without it",
            self.id,
            host::short_debug(describe, 48),
            mod_api::ABI_VERSION,
        );
    }

    fn log_slow_dispatch(&self, call: &dyn std::fmt::Debug, total: std::time::Duration) {
        const SLOW_DISPATCH: std::time::Duration = std::time::Duration::from_millis(2);
        if total < SLOW_DISPATCH {
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

    /// Readies one call. `false` means it doesn't run: a deferrable call while the mod is
    /// throttled in its context, or a store that can't be armed (which disables the mod).
    fn arm_dispatch(&mut self, call: Option<CallClass>) -> bool {
        let data = self.store.data();
        let context = Context::of(data.side, data.phase, call);
        let period = match context {
            Context::Client => Some(data.client_period),
            _ => scope::with_active_ref(|ctx| ctx.world.current_tick())
                .or_else(super::ai::detached_tick),
        };
        let budget = self.health.watchdog().budgets().context(context);
        let throttle = &mut self.store.data_mut().throttle;
        throttle.arm(context, &budget, period);
        if call.is_some_and(CallClass::deferrable) {
            let (runs, change) = throttle.admit(context);
            throttle_notice(&self.id, context, change);
            if !runs {
                return false;
            }
        }
        if let Err(e) = self.store.set_fuel(u64::MAX) {
            self.disable(&format!("arm fuel: {e:#}"));
            return false;
        }
        self.armed_fuel = u64::MAX;
        let deadline = self.store.data_mut().begin_dispatch(context);
        self.store.set_epoch_deadline(deadline);
        true
    }

    fn settle_fuel(&mut self, call: Option<&dyn std::fmt::Debug>) {
        let remaining = self.store.get_fuel().unwrap_or(0);
        let used = self.armed_fuel.saturating_sub(remaining);
        self.last_fuel = used;
        if log::log_enabled!(target: "petramond::modding::fuel", log::Level::Debug) {
            log::debug!(
                target: "petramond::modding::fuel",
                "mod '{}' used {used} fuel for {}",
                self.id,
                call.map_or_else(|| "mod_init".to_owned(), |call| host::short_debug(call, 48)),
            );
        }
        let data = self.store.data_mut();
        data.throttle.charge(data.context, used);
    }

    fn dispatch_context(&self, call: Option<&dyn std::fmt::Debug>) -> String {
        let mut out = String::new();
        if let Some(call) = call {
            out.push_str(&format!(
                " [guest call: {}]",
                host::short_debug(call, host::DIAG_DEBUG_CAP)
            ));
        }
        match &self.store.data().last_host_call() {
            Some((desc, true)) => out.push_str(&format!(" [last host call (returned): {desc}]")),
            Some((desc, false)) => out.push_str(&format!(" [host call in flight: {desc}]")),
            None => {}
        }
        out
    }

    pub(super) fn call_guest_client(
        &mut self,
        world: &crate::world::ReplicaWorld,
        call: &GuestCall,
    ) -> Option<GuestRet> {
        super::client::scope::enter(world, || self.call_guest_detached(call))
    }

    pub(super) fn set_world_seed(&mut self, seed: u32) {
        self.store.data_mut().set_world_seed(seed);
    }

    pub(super) fn client_data(&self) -> Option<&super::client::ClientStoreData> {
        self.store.data().client.as_ref()
    }

    pub(super) fn client_data_mut(&mut self) -> Option<&mut super::client::ClientStoreData> {
        self.store.data_mut().client.as_mut()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn store_data_mut(&mut self) -> &mut super::host::ModStoreData {
        self.store.data_mut()
    }

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
        if reply_len as usize > self.memory.data_size(&self.store) {
            return Err("reply exceeds guest memory".to_owned());
        }
        let mut reply = std::mem::take(&mut self.reply_buf);
        reply.clear();
        reply.resize(reply_len as usize, 0);
        let read = self
            .memory
            .read(&self.store, reply_ptr as usize, &mut reply)
            .map_err(|e| format!("read reply: {e:#}"));
        let decoded = read.and_then(|()| {
            self.fn_free
                .call(&mut self.store, (reply_ptr, reply_len))
                .map_err(|e| format!("mod_free: {e:#}"))?;
            mod_api::decode(&reply).map_err(|e| format!("malformed guest reply: {e}"))
        });
        self.reply_buf = reply;
        decoded
    }

    pub(super) fn disable(&mut self, why: &str) {
        self.health.disable(why);
    }
}

fn handshake(instance: &Instance, store: &mut Store<ModStoreData>) -> Result<AbiVersion, String> {
    if instance.get_func(&mut *store, "mod_abi_version").is_none() {
        return Err(AbiRejection::Unversioned.to_string());
    }
    let guest = instance
        .get_typed_func::<(), u32>(&mut *store, "mod_abi_version")
        .map_err(|e| format!("export mod_abi_version: {e:#}"))?
        .call(&mut *store, ())
        .map(AbiVersion::unpack)
        .map_err(|e| format!("mod_abi_version trapped: {e:#}"))?;
    let requires = match instance.get_typed_func::<(), u64>(&mut *store, "mod_abi_requires") {
        Ok(func) => func
            .call(&mut *store, ())
            .map(Capabilities::from_bits)
            .map_err(|e| format!("mod_abi_requires trapped: {e:#}"))?,
        Err(e) if guest.major == mod_api::ABI_VERSION.major => {
            return Err(format!("export mod_abi_requires: {e:#}"));
        }
        Err(_) => Capabilities::NONE,
    };
    mod_api::negotiate(
        guest,
        requires,
        mod_api::ABI_VERSION,
        mod_api::HOST_CAPABILITIES,
    )
    .map_err(|rejection| rejection.to_string())?;
    Ok(guest)
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn wat_abi_exports(version: AbiVersion) -> String {
    format!(
        "  (func (export \"mod_abi_version\") (result i32) (i32.const {}))\n  \
         (func (export \"mod_abi_requires\") (result i64) (i64.const 0))\n",
        version.pack() as i32
    )
}
