use crate::formula::{Expression, Formula, Inputs};
use serde::{Deserialize, Serialize};

/// Inclusive world bounds derived once from a placed field's parameters.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawBounds {
    pub min: [Expression; 3],
    pub max: [Expression; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl RawBounds {
    pub(super) fn compile(self, bindings: &[(String, Expression)]) -> Result<Formula, String> {
        let outputs: Vec<_> = self.min.into_iter().chain(self.max).collect();
        let formula = Formula::compile(bindings, &outputs)?;
        if formula.uses_any(&[0, 1, 2, 8]) {
            return Err("field bounds may depend only on site parameters and sea level".into());
        }
        Ok(formula)
    }
}

impl Bounds {
    pub fn intersect_formula(self, formula: &Formula, seed: u32, inputs: Inputs) -> Option<Self> {
        let values = formula.column_seeded(seed, inputs).at::<6>(0.0);
        if values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let min = std::array::from_fn(|a| self.min[a].max(values[a].ceil() as i32));
        let max = std::array::from_fn(|a| self.max[a].min(values[a + 3].floor() as i32));
        (0..3)
            .all(|a| min[a] <= max[a])
            .then_some(Self { min, max })
    }

    pub fn intersects(self, min: [i32; 3], max: [i32; 3]) -> bool {
        (0..3).all(|a| min[a] <= self.max[a] && max[a] >= self.min[a])
    }

    pub fn contains_column(self, x: i32, z: i32) -> bool {
        (self.min[0]..=self.max[0]).contains(&x) && (self.min[2]..=self.max[2]).contains(&z)
    }
}

#[cfg(test)]
mod tests;
