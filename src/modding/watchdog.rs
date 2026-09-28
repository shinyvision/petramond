//! How much a mod may use, and what happens when it wants more.
//!
//! Two mechanisms, for two different problems:
//!
//! - **Throttling** handles a mod that does too much work on average. Each context that runs on
//!   a beat (the server tick, AI decisions, client frames) gives a mod a share of every beat,
//!   with some saved-up slack for bursts. A mod that overspends isn't disabled: the work that can
//!   safely wait (its tick systems, AI decisions and client frames) is skipped until it has
//!   caught up. Work that can't be skipped without changing the game (events, block hooks,
//!   worldgen, clicks, bakes) always runs, and its cost counts toward the debt.
//! - **The watchdog** handles a mod that runs away. A mod starts with allowances for how long one
//!   call may run, how much memory it holds and how much world KV it stores. Needing more is
//!   normal, and the watchdog grants it, but gradually: at most a few times the current allowance
//!   at once, and not again until a cooldown has passed. A mod that asks again sooner, or asks
//!   for a jump far beyond what it has, is behaving like an endless loop or a leak, and is
//!   killed.
//!
//! A mod's `pack.json` can declare a tier per context and resource (`ResourceNeeds`). A tier only
//! moves the starting point, so a mod that knows it pastes castles or keeps a large map doesn't
//! have to earn its allowance first. Nobody tunes numbers: not the server owner, not the player.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mod_api::{GuestCall, RuntimeSide};
use petramond_world::pack_manifest::{ResourceNeeds, Tier};

use super::health::ModHealth;
use super::host::Phase;

/// A raise may multiply an allowance by at most this much.
pub(crate) const GROWTH_STEP: u64 = 4;

/// After a raise, the next one has to wait this long.
pub(crate) const GROWTH_COOLDOWN: Duration = Duration::from_secs(60);

const MIB: u64 = 1 << 20;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Context {
    Init,
    Tick,
    Ai,
    Client,
    Worldgen,
}

impl Context {
    const ALL: [Context; 5] = [
        Context::Init,
        Context::Tick,
        Context::Ai,
        Context::Client,
        Context::Worldgen,
    ];

    fn index(self) -> usize {
        self as usize
    }

    pub(crate) fn of(side: RuntimeSide, phase: Phase, call: Option<CallClass>) -> Self {
        if phase == Phase::Init {
            return Context::Init;
        }
        match side {
            RuntimeSide::Worldgen => Context::Worldgen,
            RuntimeSide::Client => Context::Client,
            RuntimeSide::Server if call == Some(CallClass::Ai) => Context::Ai,
            RuntimeSide::Server => Context::Tick,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Context::Init => "load-time",
            Context::Tick => "tick",
            Context::Ai => "AI",
            Context::Client => "client frame",
            Context::Worldgen => "worldgen",
        }
    }

    fn tier(self, needs: &ResourceNeeds) -> Tier {
        match self {
            Context::Init => needs.init,
            Context::Tick => needs.tick,
            Context::Ai => needs.ai,
            Context::Client => needs.client,
            Context::Worldgen => needs.worldgen,
        }
    }
}

/// What kind of call a dispatch is, as far as budgets care.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CallClass {
    /// Must run whenever the engine asks: events, block hooks, worldgen, clicks, bakes.
    Fixed,
    /// Skipping it only delays the mod: a tick system runs again next tick, and a client frame
    /// is redrawn next frame.
    Deferrable,
    /// An AI decision. Deferrable too: a node that decides nothing leaves the mob to its others.
    Ai,
}

impl CallClass {
    pub(crate) fn of(call: &GuestCall) -> Self {
        match call {
            GuestCall::AiNode { .. } | GuestCall::AiNodeBatch { .. } => CallClass::Ai,
            GuestCall::TickSystem { .. } | GuestCall::ClientFrame { .. } => CallClass::Deferrable,
            _ => CallClass::Fixed,
        }
    }

    pub(crate) fn deferrable(self) -> bool {
        self != CallClass::Fixed
    }
}

fn tier_scale(tier: Tier) -> u64 {
    match tier {
        Tier::Standard => 1,
        Tier::Heavy => 4,
        Tier::Extreme => 16,
    }
}

/// One context's allowances at the standard tier. Epochs are the engine's 50 ms watchdog beats;
/// fuel is guest instructions (plus a charge per host call), the unit the throttle counts in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextBudget {
    /// How long one call may run before the watchdog is asked for more.
    pub in_flight_epochs: u64,
    /// The share of every beat, or `None` where there is no beat to skip (load, worldgen).
    pub per_period: Option<u64>,
    /// How many beats' worth of unused share a mod may save up for a burst.
    pub burst_periods: u64,
}

impl ContextBudget {
    const fn standard(context: Context) -> Self {
        match context {
            Context::Init => Self {
                in_flight_epochs: 100,
                per_period: None,
                burst_periods: 0,
            },
            Context::Tick => Self {
                in_flight_epochs: 5,
                per_period: Some(50_000_000),
                burst_periods: 20,
            },
            Context::Ai => Self {
                in_flight_epochs: 5,
                per_period: Some(25_000_000),
                burst_periods: 20,
            },
            Context::Client => Self {
                in_flight_epochs: 4,
                per_period: Some(16_000_000),
                burst_periods: 30,
            },
            Context::Worldgen => Self {
                in_flight_epochs: 20,
                per_period: None,
                burst_periods: 0,
            },
        }
    }

    fn scaled(self, tier: Tier) -> Self {
        let k = tier_scale(tier);
        Self {
            in_flight_epochs: self.in_flight_epochs * k,
            per_period: self.per_period.map(|p| p * k),
            burst_periods: self.burst_periods,
        }
    }

    fn capacity(&self) -> Option<i64> {
        self.per_period
            .map(|p| i64::try_from(p.saturating_mul(self.burst_periods.max(1))).unwrap_or(i64::MAX))
    }
}

/// Every starting allowance for one mod, from its declared needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Budgets {
    pub contexts: [ContextBudget; 5],
    pub memory: u64,
    pub storage: u64,
}

impl Budgets {
    pub(crate) fn for_needs(needs: &ResourceNeeds) -> Self {
        Self {
            contexts: Context::ALL.map(|c| ContextBudget::standard(c).scaled(c.tier(needs))),
            memory: 128 * MIB * tier_scale(needs.memory),
            storage: MIB * tier_scale(needs.storage),
        }
    }

    pub(crate) fn context(&self, context: Context) -> ContextBudget {
        self.contexts[context.index()]
    }

    #[cfg(test)]
    pub(crate) fn with_context(mut self, context: Context, budget: ContextBudget) -> Self {
        self.contexts[context.index()] = budget;
        self
    }
}

/// Why the watchdog refused to raise an allowance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Runaway {
    /// Asked again before the cooldown after the last raise ran out.
    TooSoon,
    /// Asked for more than one raise can give.
    TooFar,
}

/// One allowance that can be raised gradually.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Grower {
    allowance: u64,
    last_raise: Option<Instant>,
}

impl Grower {
    pub(crate) fn new(allowance: u64) -> Self {
        Self {
            allowance: allowance.max(1),
            last_raise: None,
        }
    }

    pub(crate) fn allowance(&self) -> u64 {
        self.allowance
    }

    /// Makes room for `need`, raising the allowance if that is gradual enough.
    pub(crate) fn request(&mut self, need: u64, now: Instant) -> Result<(), Runaway> {
        if need <= self.allowance {
            return Ok(());
        }
        if self
            .last_raise
            .is_some_and(|at| now.saturating_duration_since(at) < GROWTH_COOLDOWN)
        {
            return Err(Runaway::TooSoon);
        }
        let room = self.allowance.saturating_mul(GROWTH_STEP);
        if need > room {
            return Err(Runaway::TooFar);
        }
        self.allowance = room;
        self.last_raise = Some(now);
        Ok(())
    }

    /// Usage the mod already had before this session (a save's stored KV) counts as granted.
    pub(crate) fn baseline(&mut self, usage: u64) {
        self.allowance = self.allowance.max(usage);
    }
}

fn runaway_reason(what: &str, why: Runaway) -> String {
    match why {
        Runaway::TooSoon => format!(
            "{what} needed more again within {}s of its last increase; stopped as a runaway",
            GROWTH_COOLDOWN.as_secs()
        ),
        Runaway::TooFar => format!(
            "{what} jumped to more than {GROWTH_STEP}x its allowance at once; stopped as a \
             runaway (a mod that needs this much up front can declare it in pack.json \
             `resources`)"
        ),
    }
}

/// One mod's allowances, shared by all of its instances.
pub(crate) struct Watchdog {
    budgets: Mutex<Budgets>,
    in_flight: Mutex<[Grower; 5]>,
    memory: Mutex<Grower>,
    memory_held: AtomicU64,
    storage: Mutex<HashMap<String, Grower>>,
}

impl Watchdog {
    pub(crate) fn new(needs: &ResourceNeeds) -> Self {
        let budgets = Budgets::for_needs(needs);
        Self {
            in_flight: Mutex::new(
                Context::ALL.map(|c| Grower::new(budgets.context(c).in_flight_epochs)),
            ),
            memory: Mutex::new(Grower::new(budgets.memory)),
            memory_held: AtomicU64::new(0),
            storage: Mutex::new(HashMap::new()),
            budgets: Mutex::new(budgets),
        }
    }

    pub(crate) fn budgets(&self) -> Budgets {
        *self.budgets.lock().unwrap()
    }

    #[cfg(test)]
    pub(crate) fn set_budgets_for_test(&self, budgets: Budgets) {
        *self.budgets.lock().unwrap() = budgets;
        *self.in_flight.lock().unwrap() =
            Context::ALL.map(|c| Grower::new(budgets.context(c).in_flight_epochs));
        *self.memory.lock().unwrap() = Grower::new(budgets.memory);
    }

    pub(crate) fn in_flight_allowance(&self, context: Context) -> u64 {
        self.in_flight.lock().unwrap()[context.index()].allowance()
    }

    /// A call has run through its allowance and wants to keep going. Returns the new allowance.
    pub(crate) fn extend_in_flight(&self, context: Context, used: u64) -> Result<u64, String> {
        let mut growers = self.in_flight.lock().unwrap();
        let grower = &mut growers[context.index()];
        grower
            .request(used.saturating_add(1), Instant::now())
            .map(|()| grower.allowance())
            .map_err(|why| runaway_reason(&format!("a {} call", context.label()), why))
    }

    pub(crate) fn grow_memory(&self, delta: u64) -> Result<(), String> {
        let mut grower = self.memory.lock().unwrap();
        let need = self
            .memory_held
            .load(Ordering::Acquire)
            .saturating_add(delta);
        grower
            .request(need, Instant::now())
            .map_err(|why| runaway_reason("its memory", why))?;
        self.memory_held.fetch_add(delta, Ordering::AcqRel);
        Ok(())
    }

    pub(crate) fn release_memory(&self, bytes: u64) {
        self.memory_held.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// A world KV write takes `namespace` from `before` to `after` stored bytes.
    pub(crate) fn grow_storage(
        &self,
        namespace: &str,
        before: u64,
        after: u64,
    ) -> Result<(), String> {
        let start = self.budgets().storage;
        let mut storage = self.storage.lock().unwrap();
        let grower = storage.entry(namespace.to_owned()).or_insert_with(|| {
            let mut grower = Grower::new(start);
            grower.baseline(before);
            grower
        });
        grower
            .request(after, Instant::now())
            .map_err(|why| runaway_reason(&format!("its world KV in '{namespace}'"), why))
    }
}

/// The needs a mod's installed pack declares; a mod with no pack (tests, fixtures) gets none.
pub(crate) fn declared_needs(mod_id: &str) -> ResourceNeeds {
    petramond_world::assets::packs()
        .iter()
        .find(|pack| pack.id.as_deref() == Some(mod_id))
        .map(|pack| pack.resources)
        .unwrap_or_default()
}

/// Per-instance throttle: one bucket of fuel per context that runs on a beat.
pub(crate) struct Throttle {
    buckets: [Bucket; 5],
}

#[derive(Copy, Clone, Debug, Default)]
struct Bucket {
    tokens: i64,
    period: Option<u64>,
    paused: bool,
}

/// What a charge or a skipped call changed, for logging once per episode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ThrottleChange {
    None,
    Paused,
    Resumed,
}

impl Throttle {
    pub(crate) fn new(budgets: &Budgets) -> Self {
        let mut buckets = [Bucket::default(); 5];
        for c in Context::ALL {
            buckets[c.index()].tokens = budgets.context(c).capacity().unwrap_or(0);
        }
        Self { buckets }
    }

    /// Credits the beats that passed since the context last ran.
    pub(crate) fn arm(&mut self, context: Context, budget: &ContextBudget, period: Option<u64>) {
        let (Some(per), Some(cap), Some(now)) = (budget.per_period, budget.capacity(), period)
        else {
            return;
        };
        let bucket = &mut self.buckets[context.index()];
        bucket.tokens = match bucket.period {
            Some(last) if now >= last => {
                let earned = (now - last).saturating_mul(per);
                bucket
                    .tokens
                    .saturating_add(i64::try_from(earned).unwrap_or(i64::MAX))
                    .min(cap)
            }
            // The beat went backwards: a new world or session started counting afresh.
            Some(_) => cap,
            None => bucket.tokens.min(cap),
        };
        bucket.period = Some(now);
    }

    /// Whether a deferrable call may run now; the first refusal and the first run after catching
    /// up are reported so the log shows each throttling episode once.
    pub(crate) fn admit(&mut self, context: Context) -> (bool, ThrottleChange) {
        let bucket = &mut self.buckets[context.index()];
        let runs = bucket.tokens > 0;
        let change = match (runs, bucket.paused) {
            (false, false) => ThrottleChange::Paused,
            (true, true) => ThrottleChange::Resumed,
            _ => ThrottleChange::None,
        };
        bucket.paused = !runs;
        (runs, change)
    }

    pub(crate) fn charge(&mut self, context: Context, fuel: u64) {
        let bucket = &mut self.buckets[context.index()];
        bucket.tokens = bucket
            .tokens
            .saturating_sub(i64::try_from(fuel).unwrap_or(i64::MAX));
    }

    #[cfg(test)]
    pub(crate) fn paused(&self, context: Context) -> bool {
        self.buckets[context.index()].tokens <= 0
    }
}

pub(crate) fn throttle_notice(mod_id: &str, context: Context, change: ThrottleChange) {
    match change {
        ThrottleChange::Paused => log::warn!(
            "mod '{mod_id}' is using more than its share of {} time; its {} work is paused \
             until it catches up",
            context.label(),
            context.label()
        ),
        ThrottleChange::Resumed => log::info!(
            "mod '{mod_id}' caught up; its {} work runs again",
            context.label()
        ),
        ThrottleChange::None => {}
    }
}

/// The memory limiter of one instance: every growth asks the mod's watchdog.
pub(crate) struct MemoryGuard {
    health: Arc<ModHealth>,
    held: u64,
}

impl MemoryGuard {
    pub(crate) fn new(health: Arc<ModHealth>) -> Self {
        Self { health, held: 0 }
    }
}

const TABLE_ELEMENTS_MAX: usize = 1 << 20;

impl wasmtime::ResourceLimiter for MemoryGuard {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if maximum.is_some_and(|max| desired > max) {
            return Ok(false);
        }
        let delta = desired.saturating_sub(current) as u64;
        self.health
            .watchdog()
            .grow_memory(delta)
            .map_err(wasmtime::Error::msg)?;
        self.held += delta;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= maximum.unwrap_or(usize::MAX).min(TABLE_ELEMENTS_MAX))
    }
}

impl Drop for MemoryGuard {
    fn drop(&mut self) {
        self.health.watchdog().release_memory(self.held);
    }
}

#[cfg(test)]
mod tests;
