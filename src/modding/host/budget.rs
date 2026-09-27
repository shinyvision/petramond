use mod_api::RuntimeSide;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuelBudget {
    pub per_dispatch: u64,
    pub per_tick: u64,
}

impl FuelBudget {
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

pub(in crate::modding) const HOST_CALL_BASE_FUEL: u64 = 1_000;

pub(in crate::modding) const HOST_CALL_FUEL_PER_BYTE: u64 = 4;

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

#[derive(Clone, Copy, Debug)]
pub(in crate::modding) struct TickMeter {
    budget: FuelBudget,
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

    pub(in crate::modding) fn arm(&mut self, tick: Option<u64>) -> u64 {
        if self.tick != tick {
            self.tick = tick;
            self.used = 0;
        }
        u64::MAX
    }

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

    #[cfg_attr(not(test), allow(dead_code))]
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
