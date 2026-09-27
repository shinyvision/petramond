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

/// Whether the per-dispatch wall-time diagnostic is switched on (see
/// [`ModInstance::log_slow_dispatch`]).
#[inline]
fn slow_dispatch_logging() -> bool {
    log::log_enabled!(target: "petramond::modding::perf", log::Level::Debug)
}

use super::health::ModHealth;
use super::host::budget::FuelBudget;
use super::host::{self, ModStoreData, Phase, Registration, DISPATCH_DEADLINE_EPOCHS};
use super::scope;

pub(super) struct ModInstance {
    id: String,
    store: Store<ModStoreData>,
    memory: Memory,
    fn_init: TypedFunc<(u32, u64), ()>,
    fn_alloc: TypedFunc<u32, u32>,
    fn_free: TypedFunc<(u32, u32), ()>,
    fn_dispatch: TypedFunc<(u32, u32), u64>,
    /// The mod's session-wide health, shared with its other instances.
    health: Arc<ModHealth>,
    /// Fuel the current guest entry was armed with (see [`host::budget`]).
    armed_fuel: u64,
    /// Successful guest dispatches (init + tick systems + events), for tests
    /// and diagnostics.
    dispatches: u64,
    /// Reused request/reply staging. A dispatch is the engine's most frequent
    /// mod interaction — every AI node of every mob, every tick, among others
    /// — and each one used to allocate a fresh `Vec` for the encoded request
    /// and another (zero-filled) for the reply.
    request_buf: Vec<u8>,
    reply_buf: Vec<u8>,
    /// Call kinds this guest has already declined as unsupported — logged
    /// once each, not on every dispatch.
    declined: Vec<std::mem::Discriminant<GuestCall>>,
}

impl ModInstance {
    /// A standalone server instance with its own health and the default
    /// fuel budget (fixtures and single-instance tools).
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn from_module(id: &str, module: &Module, world_seed: u32) -> Result<Self, String> {
        Self::from_module_side(
            id,
            module,
            world_seed,
            mod_api::RuntimeSide::Server,
            None,
            ModHealth::standalone(id),
            FuelBudget::DEFAULT,
        )
    }

    pub(super) fn from_module_side(
        id: &str,
        module: &Module,
        world_seed: u32,
        side: mod_api::RuntimeSide,
        client_buckets: Option<super::client::ClientBuckets>,
        health: Arc<ModHealth>,
        budget: FuelBudget,
    ) -> Result<Self, String> {
        let mut store = Store::new(
            host::engine(),
            ModStoreData::new_for_side(id, world_seed, side, client_buckets),
        );
        store.data_mut().meter.set_budget(budget);
        // Instantiation runs guest code too (data/start sections): same leash.
        store
            .set_fuel(u64::MAX)
            .map_err(|e| format!("arm fuel: {e:#}"))?;
        store.set_epoch_deadline(DISPATCH_DEADLINE_EPOCHS);
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
            armed_fuel: budget.per_dispatch,
            dispatches: 0,
        })
    }

    pub(super) fn disabled(&self) -> bool {
        self.health.is_disabled()
    }

    /// Replace this instance's fuel budgets (the session's configuration).
    pub(super) fn set_fuel_budget(&mut self, budget: FuelBudget) {
        self.store.data_mut().meter.set_budget(budget);
    }

    #[cfg_attr(not(test), allow(dead_code))] // test observability
    pub(super) fn fuel_used_this_tick(&self) -> u64 {
        self.store.data().meter.used_this_tick()
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
    /// any sim-scoped host call gets `HostRet::Err`, so a gen-hook mod's init
    /// must stay pure (registrations, `ResolveBlock`, `Log`, `RngU64`).
    pub(super) fn call_init_detached(&mut self) {
        debug_assert!(self.store.data().phase == Phase::Init);
        if !self.arm_dispatch() {
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

    /// The registrations `mod_init` collected — empty if the mod got disabled.
    pub(super) fn take_registrations(&mut self) -> Vec<Registration> {
        if self.disabled() {
            self.store.data_mut().pending.clear();
            return Vec::new();
        }
        std::mem::take(&mut self.store.data_mut().pending)
    }

    /// Dispatch one [`GuestCall`], publishing `ctx` for re-entrant host calls.
    /// `None` = no answer: the mod is (or just became) disabled, or it declined
    /// the call as unsupported; the caller carries on either way.
    pub(super) fn call_guest(&mut self, ctx: &mut SimCtx, call: &GuestCall) -> Option<GuestRet> {
        scope::enter(ctx, || self.call_guest_detached(call))
    }

    /// [`call_guest`](Self::call_guest) with a READ-ONLY scope: the guest may
    /// query the world but a mutating host call errors. The shape
    /// placement-plan dispatch path (the ABI promises read-only access).
    pub(super) fn call_guest_read_only(
        &mut self,
        ctx: &mut SimCtx,
        call: &GuestCall,
    ) -> Option<GuestRet> {
        scope::enter_read_only(ctx, || self.call_guest_detached(call))
    }

    /// [`call_guest`](Self::call_guest) WITHOUT publishing a simulation
    /// context — the worldgen dispatch path (worker threads and any thread
    /// running `generate_*`): sim-scoped host calls made during the dispatch
    /// are rejected, everything else (deadline, disable-on-trap, protocol)
    /// behaves identically.
    pub(super) fn call_guest_detached(&mut self, call: &GuestCall) -> Option<GuestRet> {
        self.call_guest_encoded(call, std::mem::discriminant(call), call)
    }

    /// [`call_guest_detached`](Self::call_guest_detached) for any encoding of
    /// a guest call: the owned [`GuestCall`], or a borrowed view serializing
    /// to exactly its bytes (the batched AI dispatch ships each mob's tag map
    /// straight from the mob that way). `kind` is the variant those bytes
    /// decode as and `describe` what diagnostics print for the call.
    pub(super) fn call_guest_encoded<C: serde::Serialize>(
        &mut self,
        call: &C,
        kind: std::mem::Discriminant<GuestCall>,
        describe: &dyn std::fmt::Debug,
    ) -> Option<GuestRet> {
        if self.disabled() {
            return None;
        }
        let mut request = std::mem::take(&mut self.request_buf);
        let request_len = match mod_api::encode_into(call, &mut request) {
            Ok(len) => len,
            Err(e) => {
                // Host-side bug, but never let it poison the sim either.
                self.request_buf = request;
                self.disable(&format!("encode guest call: {e}"));
                return None;
            }
        };
        if !self.arm_dispatch() {
            self.request_buf = request;
            return None;
        }
        // Only timed when the diagnostic that reads it is switched on: a clock
        // read per dispatch is real cost on a path that runs thousands of
        // times a tick.
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

    /// Whether this guest has declined calls of `kind` as unsupported — a
    /// caller with an older equivalent (the per-mob AI dispatch) goes
    /// straight to it instead of asking again every tick.
    pub(super) fn declines(&self, kind: std::mem::Discriminant<GuestCall>) -> bool {
        self.declined.contains(&kind)
    }

    /// The guest answered [`GuestRet::Unsupported`]: it was built against an
    /// older ABI minor that predates this call. Say so once per call kind.
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

    /// Perf diagnostics: any dispatch over the threshold logs its guest/host
    /// wall split under the `petramond::modding::perf` target, so frame
    /// stutter attributes to the mod, the call, and the side of the ABI it
    /// spent its time on.
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

    /// Arm one guest entry with the full meter range, the emergency
    /// wall-clock backstop, and per-dispatch host-call accounting.
    ///
    /// The tick is the published simulation context's, or the detached AI
    /// dispatch's; worldgen and client instances have no tick counter.
    fn arm_dispatch(&mut self) -> bool {
        let tick = scope::with_active_ref(|ctx| ctx.world.current_tick())
            .or_else(super::ai::detached_tick);
        let fuel = self.store.data_mut().meter.arm(tick);
        if let Err(e) = self.store.set_fuel(fuel) {
            self.disable(&format!("arm fuel: {e:#}"));
            return false;
        }
        self.armed_fuel = fuel;
        self.store.set_epoch_deadline(DISPATCH_DEADLINE_EPOCHS);
        self.store.data_mut().begin_dispatch();
        true
    }

    /// Charge the finished entry and warn once per session mod when a
    /// diagnostic threshold was crossed.
    fn settle_fuel(&mut self, call: Option<&dyn std::fmt::Debug>) {
        let remaining = self.store.get_fuel().unwrap_or(0);
        let used = self.armed_fuel.saturating_sub(remaining);
        if log::log_enabled!(target: "petramond::modding::fuel", log::Level::Debug) {
            log::debug!(
                target: "petramond::modding::fuel",
                "mod '{}' used {used} fuel for {}",
                self.id,
                call.map_or_else(|| "mod_init".to_owned(), |call| host::short_debug(call, 48)),
            );
        }
        let meter = &mut self.store.data_mut().meter;
        if let Some(warning) = meter.charge(self.armed_fuel, remaining) {
            self.health.warn_fuel_once(&warning);
        }
    }

    /// Diagnostic suffix for disable messages: the guest call that was in
    /// flight and the dispatch's most recent host call — a failure names what
    /// was actually happening instead of just the trap kind.
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

    /// See [`ModStoreData::set_world_seed`](super::host::ModStoreData::set_world_seed).
    pub(super) fn set_world_seed(&mut self, seed: u32) {
        self.store.data_mut().set_world_seed(seed);
    }

    pub(super) fn client_data(&self) -> Option<&super::client::ClientStoreData> {
        self.store.data().client.as_ref()
    }

    pub(super) fn client_data_mut(&mut self) -> Option<&mut super::client::ClientStoreData> {
        self.store.data_mut().client.as_mut()
    }

    /// The whole store, for issuing a host call as this instance (tests).
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn store_data_mut(&mut self) -> &mut super::host::ModStoreData {
        self.store.data_mut()
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

    /// Session-scoped kill switch: one visible error line, then the mod —
    /// THIS instance and every other instance sharing its health — stops
    /// receiving dispatches until the next launch.
    pub(super) fn disable(&mut self, why: &str) {
        self.health.disable(why);
    }
}

/// Read the guest's side of the ABI handshake and check it against this host.
/// Runs before `mod_init`, so an incompatible module is refused at load with
/// the reason spelled out — never half-initialized, never trapping mid-game.
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
        // Only a same-major guest is bound to this ABI's export set; another
        // major is refused on its version alone.
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

/// The handshake exports a hand-written WAT test guest needs: declares
/// `version` and requires no capabilities. `mod_init` then takes the host's
/// `(param i32 i64)`.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn wat_abi_exports(version: AbiVersion) -> String {
    format!(
        "  (func (export \"mod_abi_version\") (result i32) (i32.const {}))\n  \
         (func (export \"mod_abi_requires\") (result i64) (i64.const 0))\n",
        version.pack() as i32
    )
}
