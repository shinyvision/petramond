use super::{ConditionDef, ConditionId, MAX_STAGES};

/// One condition active on a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveCondition {
    pub condition: ConditionId,
    /// Ticks each stage stays reached, counted down together; the strongest
    /// stage with time left is the current one.
    holds: [u32; MAX_STAGES],
    elapsed: u32,
    /// Ticks until the next pulse; `0` while no damaging stage has run a clock.
    pulse_in: u32,
}

impl ActiveCondition {
    /// The current (strongest remaining) stage.
    pub fn stage(&self) -> u8 {
        self.holds.iter().rposition(|&h| h > 0).unwrap_or(0) as u8
    }

    /// Ticks until the condition ends.
    pub fn remaining(&self) -> u32 {
        self.holds.iter().copied().max().unwrap_or(0)
    }

    /// Ticks since the condition began.
    pub fn elapsed(&self) -> u32 {
        self.elapsed
    }

    fn cool(&mut self, ticks: u32) {
        for hold in &mut self.holds {
            *hold = hold.saturating_sub(ticks);
        }
    }
}

/// Damage due from a condition this tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ConditionPulse {
    pub condition: ConditionId,
    pub amount: i32,
}

/// Every condition on one body, ordered by id so ticking is deterministic.
/// Transient: bodies start every life without conditions. Mutation is
/// crate-private: every grant outside this crate goes through
/// [`BodyExposure`](crate::exposure::BodyExposure), which owns what a body refuses.
#[derive(Clone, Debug, Default)]
pub struct BodyConditions {
    active: Vec<ActiveCondition>,
}

impl BodyConditions {
    pub fn active(&self) -> &[ActiveCondition] {
        &self.active
    }

    pub fn get(&self, condition: ConditionId) -> Option<&ActiveCondition> {
        self.index(condition).ok().map(|i| &self.active[i])
    }

    fn index(&self, condition: ConditionId) -> Result<usize, usize> {
        self.active
            .binary_search_by_key(&condition, |c| c.condition)
    }

    /// Grant `ticks` at `stage`. Fuel extends by `max`, stages only upgrade, and an
    /// already-running pulse clock is never reset; a stage with `cools_to` holds
    /// for its `cools_after` fraction of the grant and the rest is granted to the
    /// weaker stage. `false` when nothing was granted (no ticks, no such stage).
    pub(crate) fn apply(&mut self, def: &ConditionDef, stage: u8, ticks: u32) -> bool {
        if ticks == 0 || stage as usize >= def.stages.len() {
            return false;
        }
        let at = self.index(def.id).unwrap_or_else(|at| {
            self.active.insert(
                at,
                ActiveCondition {
                    condition: def.id,
                    holds: [0; MAX_STAGES],
                    elapsed: 0,
                    pulse_in: 0,
                },
            );
            at
        });
        let entry = &mut self.active[at];
        let mut s = stage as usize;
        loop {
            let row = &def.stages[s];
            let Some(to) = row.cools_to else {
                entry.holds[s] = entry.holds[s].max(ticks);
                break;
            };
            let held = (f64::from(ticks) * f64::from(row.cools_after)).ceil() as u32;
            entry.holds[s] = entry.holds[s].max(held);
            s = to as usize;
        }
        true
    }

    /// Consume `ticks` of a condition's time without moving its pulse boundary.
    /// `u32::MAX` clears it.
    pub(crate) fn cool(&mut self, condition: ConditionId, ticks: u32) {
        if let Ok(at) = self.index(condition) {
            self.active[at].cool(ticks);
            if self.active[at].remaining() == 0 {
                self.active.remove(at);
            }
        }
    }

    pub(crate) fn clear(&mut self, condition: ConditionId) {
        if let Ok(at) = self.index(condition) {
            self.active.remove(at);
        }
    }

    /// Advance one fixed tick, reporting each damage pulse due. The pulse clock
    /// is shared across stage changes and pays the CURRENT stage's damage; a
    /// damaging stage reached with no clock running starts one a full interval
    /// out, so neither a fresh condition nor an upgrade pulses early.
    pub(crate) fn tick(&mut self, defs: &[ConditionDef], mut on_pulse: impl FnMut(ConditionPulse)) {
        for entry in &mut self.active {
            let Some(def) = defs.get(entry.condition.0 as usize) else {
                entry.cool(u32::MAX);
                continue;
            };
            entry.elapsed = entry.elapsed.saturating_add(1);
            match def.stages[entry.stage() as usize].damage {
                None => entry.pulse_in = 0,
                Some(pulse) => {
                    if entry.pulse_in == 0 {
                        entry.pulse_in = pulse.interval;
                    }
                    entry.pulse_in -= 1;
                    if entry.pulse_in == 0 {
                        on_pulse(ConditionPulse {
                            condition: entry.condition,
                            amount: pulse.amount,
                        });
                        entry.pulse_in = pulse.interval;
                    }
                }
            }
            entry.cool(1);
        }
        self.active.retain(|c| c.remaining() > 0);
    }
}
