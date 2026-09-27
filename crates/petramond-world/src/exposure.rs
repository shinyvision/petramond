use crate::block::Block;
use crate::condition::{BodyConditions, ConditionDef, ConditionId};
use crate::fluid::FluidDef;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExposureSource {
    Fluid(Block),
    Condition(ConditionId),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExposureDamage {
    pub amount: i32,
    pub source: ExposureSource,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Tolerance {
    pub blocks: &'static [Block],
    pub conditions: &'static [ConditionId],
}

#[derive(Clone, Debug, Default)]
pub struct BodyExposure {
    conditions: BodyConditions,
    tolerance: Tolerance,
    contact_in: Vec<(Block, u32)>,
    doused: Vec<ConditionId>,
}

impl BodyExposure {
    pub fn new(tolerance: Tolerance) -> Self {
        Self {
            tolerance,
            ..Self::default()
        }
    }

    pub fn conditions(&self) -> &BodyConditions {
        &self.conditions
    }

    pub fn refuses(&self, condition: ConditionId) -> bool {
        self.tolerance.conditions.contains(&condition)
            || self.doused.binary_search(&condition).is_ok()
    }

    pub fn apply(&mut self, def: &ConditionDef, stage: u8, ticks: u32) -> bool {
        !self.refuses(def.id) && self.conditions.apply(def, stage, ticks)
    }

    pub fn cool(&mut self, condition: ConditionId, ticks: u32) {
        self.conditions.cool(condition, ticks);
    }

    pub fn tick(
        &mut self,
        touched: Option<&[&'static FluidDef]>,
        defs: &[ConditionDef],
        out: &mut Vec<ExposureDamage>,
    ) {
        for (_, clock) in &mut self.contact_in {
            *clock = clock.saturating_sub(1);
        }
        if let Some(touched) = touched {
            debug_assert!(
                touched
                    .windows(2)
                    .all(|w| w[0].block.id() < w[1].block.id()),
                "touched fluids must be sorted and unique by block id"
            );
            self.contact(touched, defs, out);
        }
        self.contact_in.retain(|(_, clock)| *clock > 0);
        self.conditions.tick(defs, |pulse| {
            out.push(ExposureDamage {
                amount: pulse.amount,
                source: ExposureSource::Condition(pulse.condition),
            })
        });
    }

    fn contact(
        &mut self,
        touched: &[&'static FluidDef],
        defs: &[ConditionDef],
        out: &mut Vec<ExposureDamage>,
    ) {
        let tolerated = self.tolerance.blocks;
        let felt = || {
            touched
                .iter()
                .copied()
                .filter(|f| !tolerated.contains(&f.block))
        };
        for fluid in felt() {
            let Some(pulse) = fluid.contact.damage else {
                continue;
            };
            let at = self
                .contact_in
                .binary_search_by_key(&fluid.block.id(), |(b, _)| b.id());
            match at {
                Ok(at) if self.contact_in[at].1 > 0 => continue,
                Ok(at) => self.contact_in[at].1 = pulse.interval,
                Err(at) => self.contact_in.insert(at, (fluid.block, pulse.interval)),
            }
            out.push(ExposureDamage {
                amount: pulse.amount,
                source: ExposureSource::Fluid(fluid.block),
            });
        }
        self.doused.clear();
        for fluid in felt() {
            self.doused.extend_from_slice(fluid.contact.clears);
        }
        self.doused.sort_unstable();
        self.doused.dedup();
        for &id in &self.doused {
            self.conditions.clear(id);
        }
        for grant in felt().filter_map(|f| f.contact.applies) {
            if let Some(def) = defs.get(grant.condition.0 as usize) {
                self.apply(def, grant.stage, grant.ticks);
            }
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new(self.tolerance);
    }
}

#[cfg(test)]
mod tests;
