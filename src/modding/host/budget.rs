//! Deterministic resource metering for guest code.
//!
//! Every guest entry runs on wasmtime FUEL — a count of executed wasm
//! instructions, identical on every machine for the same guest and inputs —
//! instead of wall time. Two budgets apply, both configurable per session
//! ([`FuelBudget`], set through `ModHost::set_fuel_budget`):
//!
//! - **per dispatch**: what one guest entry (an event, a tick system, an AI
//!   node, a gen hook) may burn before it traps;
//! - **per tick**: what one mod may burn across ALL its dispatches within one
//!   simulation tick. Thousands of cheap dispatches (an AI node per mob) can
//!   no longer each stay under a generous per-dispatch limit and together
//!   stall the tick loop.
//!
//! Host calls are charged too ([`host_call_fuel`]): a fixed cost per call,
//! plus, on the deterministic sides (server, worldgen), a per-byte cost over
//! the request and the reply, so host-side per-element work (a maximal
//! `GetBlocks`) is metered in the same currency and just as
//! deterministically. A client instance pays the fixed cost only: nothing
//! there must disable identically on every machine, its element queries are
//! capped per call, and its large payloads (a file read, a pushed video
//! frame) are copies whose only bound is what the guest can address.
//!
//! Running out of either budget disables the mod for the session with a
//! reason naming the budget. Because fuel is deterministic, a world that
//! disables a mod on one machine disables it on every machine at the same
//! dispatch. The wall-clock epoch deadline (`DISPATCH_DEADLINE_EPOCHS`) stays
//! only as a last-resort backstop against a hang the meter cannot see.
//!
//! Instances with no simulation tick (worldgen workers, client presentation)
//! are held to the per-dispatch budget only.

use mod_api::RuntimeSide;

/// A session's fuel budgets (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuelBudget {
    /// Fuel one guest entry may burn.
    pub per_dispatch: u64,
    /// Fuel one mod may burn across every dispatch of one simulation tick.
    pub per_tick: u64,
}

impl FuelBudget {
    /// Orders of magnitude above legitimate use — the heaviest bundled
    /// dispatches (the builder's planners) burn a few million — while a
    /// runaway loop exhausts it in well under a second of guest compute.
    pub const DEFAULT: Self = Self {
        per_dispatch: 200_000_000,
        per_tick: 1_000_000_000,
    };
}

impl Default for FuelBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Fixed fuel charged for every host call.
pub(in crate::modding) const HOST_CALL_BASE_FUEL: u64 = 1_000;

/// Fuel charged per byte of a host call's request and reply — the
/// deterministic proxy for the host's per-element work.
pub(in crate::modding) const HOST_CALL_FUEL_PER_BYTE: u64 = 4;

/// The fuel one host call costs a guest on `side`.
pub(in crate::modding) fn host_call_fuel(
    side: RuntimeSide,
    request_len: usize,
    reply_len: usize,
) -> u64 {
    if side == RuntimeSide::Client {
        return HOST_CALL_BASE_FUEL;
    }
    let bytes = (request_len as u64).saturating_add(reply_len as u64);
    HOST_CALL_BASE_FUEL.saturating_add(bytes.saturating_mul(HOST_CALL_FUEL_PER_BYTE))
}

/// One instance's fuel accounting: the budget, and what the mod burned in
/// the tick it last dispatched in.
#[derive(Clone, Copy, Debug)]
pub(in crate::modding) struct TickMeter {
    budget: FuelBudget,
    /// The simulation tick `used` belongs to.
    tick: Option<u64>,
    used: u64,
}

impl TickMeter {
    pub(in crate::modding) fn new(budget: FuelBudget) -> Self {
        Self {
            budget,
            tick: None,
            used: 0,
        }
    }

    pub(in crate::modding) fn set_budget(&mut self, budget: FuelBudget) {
        self.budget = budget;
    }

    /// The fuel to arm a dispatch with during simulation tick `tick`
    /// (`None` = no tick: per-dispatch budget only), or why the mod may not
    /// run at all because this tick's budget is already spent.
    pub(in crate::modding) fn arm(&mut self, tick: Option<u64>) -> Result<u64, String> {
        let Some(tick) = tick else {
            return Ok(self.budget.per_dispatch);
        };
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.used = 0;
        }
        let left = self.budget.per_tick.saturating_sub(self.used);
        if left == 0 {
            return Err(self.tick_exhausted());
        }
        Ok(left.min(self.budget.per_dispatch))
    }

    /// Record a finished dispatch that was armed with `armed` fuel and left
    /// `remaining` unspent.
    pub(in crate::modding) fn charge(&mut self, armed: u64, remaining: u64) {
        if self.tick.is_some() {
            self.used = self.used.saturating_add(armed.saturating_sub(remaining));
        }
    }

    /// Fuel burned so far in the current tick.
    #[cfg_attr(not(test), allow(dead_code))] // test observability
    pub(in crate::modding) fn used_this_tick(&self) -> u64 {
        self.used
    }

    /// Why a dispatch armed with `armed` fuel ran dry: the per-dispatch
    /// budget, or what was left of this tick's.
    pub(in crate::modding) fn exhaustion(&self, armed: u64) -> String {
        if armed < self.budget.per_dispatch && self.tick.is_some() {
            self.tick_exhausted()
        } else {
            format!(
                "exhausted its per-dispatch fuel budget ({} units)",
                self.budget.per_dispatch
            )
        }
    }

    fn tick_exhausted(&self) -> String {
        format!(
            "exhausted its per-tick fuel budget ({} units in tick {})",
            self.budget.per_tick,
            self.tick.unwrap_or_default()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: FuelBudget = FuelBudget {
        per_dispatch: 100,
        per_tick: 250,
    };

    #[test]
    fn tickless_dispatches_get_the_per_dispatch_budget_only() {
        let mut meter = TickMeter::new(SMALL);
        for _ in 0..10 {
            assert_eq!(meter.arm(None), Ok(100));
            meter.charge(100, 0);
        }
        assert_eq!(meter.used_this_tick(), 0);
        assert!(meter.exhaustion(100).contains("per-dispatch"));
    }

    #[test]
    fn the_tick_budget_spans_every_dispatch_of_a_tick() {
        let mut meter = TickMeter::new(SMALL);
        assert_eq!(meter.arm(Some(7)), Ok(100));
        meter.charge(100, 0);
        assert_eq!(meter.arm(Some(7)), Ok(100));
        meter.charge(100, 10);
        // 190 burned: the third dispatch is armed with what is left.
        assert_eq!(meter.arm(Some(7)), Ok(60));
        meter.charge(60, 0);
        assert!(meter.exhaustion(60).contains("per-tick"));
        let refused = meter.arm(Some(7)).unwrap_err();
        assert!(refused.contains("per-tick") && refused.contains("tick 7"));
        // A new tick starts a fresh budget.
        assert_eq!(meter.arm(Some(8)), Ok(100));
        assert_eq!(meter.used_this_tick(), 0);
    }

    #[test]
    fn host_calls_cost_a_base_plus_their_bytes() {
        let server = RuntimeSide::Server;
        assert_eq!(host_call_fuel(server, 0, 0), HOST_CALL_BASE_FUEL);
        assert_eq!(
            host_call_fuel(server, 10, 6),
            HOST_CALL_BASE_FUEL + 16 * HOST_CALL_FUEL_PER_BYTE
        );
        assert_eq!(host_call_fuel(server, usize::MAX, usize::MAX), u64::MAX);
    }

    /// A client's bulk reply (a whole recording read in one answer) is not a
    /// size cap in disguise: it costs what an empty call costs.
    #[test]
    fn a_client_read_larger_than_the_dispatch_budget_costs_the_base() {
        let reply = (FuelBudget::DEFAULT.per_dispatch as usize) * 2;
        assert_eq!(
            host_call_fuel(RuntimeSide::Client, 64, reply),
            HOST_CALL_BASE_FUEL
        );
        assert!(
            host_call_fuel(RuntimeSide::Worldgen, 64, reply) > FuelBudget::DEFAULT.per_dispatch
        );
    }
}
