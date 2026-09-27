//! Deterministic diagnostics for guest code.
//!
//! Every guest entry runs on wasmtime FUEL — a count of executed wasm
//! instructions, identical on every machine for the same guest and inputs.
//! Two warning thresholds apply, both configurable per session
//! ([`FuelBudget`], set through `ModHost::set_fuel_budget`):
//!
//! - **per dispatch**: a costly guest entry (event, tick system, AI node, gen hook);
//! - **per tick**: costly combined work across a mod's simulation tick.
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
//! Crossing a threshold warns once per mod and session. The guest keeps
//! running; the wall-clock epoch deadline (`DISPATCH_DEADLINE_EPOCHS`) stays
//! as a last-resort backstop against a genuinely unbounded guest loop.
//!
//! Instances with no simulation tick (worldgen workers, client presentation)
//! are measured against the per-dispatch threshold only.

use mod_api::RuntimeSide;

/// A session's fuel warning thresholds (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuelBudget {
    /// Fuel one guest entry may burn before a warning.
    pub per_dispatch: u64,
    /// Fuel one mod may burn across one simulation tick before a warning.
    pub per_tick: u64,
}

impl FuelBudget {
    /// Diagnostic thresholds for unusually expensive mod work.
    pub const DEFAULT: Self = Self {
        per_dispatch: 1_000_000_000,
        per_tick: 5_000_000_000,
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

/// One instance's fuel accounting: the thresholds, and what the mod burned in
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

    /// Start accounting for `tick`; `None` is a tickless client or worldgen
    /// dispatch. The store gets the full Wasmtime fuel range so a warning
    /// threshold cannot interrupt a guest call.
    pub(in crate::modding) fn arm(&mut self, tick: Option<u64>) -> u64 {
        if self.tick != tick {
            self.tick = tick;
            self.used = 0;
        }
        u64::MAX
    }

    /// Record a finished dispatch that was armed with `armed` fuel and left
    /// `remaining` unspent.
    pub(in crate::modding) fn charge(&mut self, armed: u64, remaining: u64) -> Option<String> {
        let dispatch = armed.saturating_sub(remaining);
        if self.tick.is_some() {
            self.used = self.used.saturating_add(dispatch);
        }
        if dispatch > self.budget.per_dispatch {
            Some(format!(
                "dispatch used {dispatch} fuel, above the {} warning threshold",
                self.budget.per_dispatch
            ))
        } else if self.tick.is_some() && self.used > self.budget.per_tick {
            Some(format!(
                "tick {} used {} fuel, above the {} warning threshold",
                self.tick.unwrap_or_default(),
                self.used,
                self.budget.per_tick
            ))
        } else {
            None
        }
    }

    /// Fuel burned so far in the current tick.
    #[cfg_attr(not(test), allow(dead_code))] // test observability
    pub(in crate::modding) fn used_this_tick(&self) -> u64 {
        self.used
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
    fn tickless_dispatches_warn_on_costly_entries() {
        let mut meter = TickMeter::new(SMALL);
        let armed = meter.arm(None);
        assert_eq!(armed, u64::MAX);
        assert!(meter
            .charge(armed, armed - 101)
            .unwrap()
            .contains("dispatch"));
        assert_eq!(meter.used_this_tick(), 0);
    }

    #[test]
    fn tick_warnings_accumulate_without_refusing_dispatches() {
        let mut meter = TickMeter::new(SMALL);
        let armed = meter.arm(Some(7));
        assert_eq!(meter.charge(armed, armed - 90), None);
        assert_eq!(meter.arm(Some(7)), u64::MAX);
        assert_eq!(meter.charge(armed, armed - 90), None);
        assert!(meter.charge(armed, armed - 90).unwrap().contains("tick 7"));
        assert_eq!(meter.arm(Some(8)), u64::MAX);
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
