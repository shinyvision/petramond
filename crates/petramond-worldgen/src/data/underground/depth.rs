use crate::formula::{Expression, Formula, Inputs};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_FLOOR_DEPTH: i32 = 16;

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum RawDepth {
    Constant(i32),
    Pattern(DepthExpression),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DepthExpression {
    max: i32,
    #[serde(default)]
    bindings: Vec<(String, Expression)>,
    extent: Expression,
}

impl Default for RawDepth {
    fn default() -> Self {
        Self::Constant(1)
    }
}

#[derive(Debug)]
struct DepthPattern {
    formula: Formula,
    fingerprint: u64,
}

/// A bounded floor course, with an optional expression sampled at its top cell.
#[derive(Clone, Copy, Debug)]
pub struct FloorDepth {
    max: i32,
    pattern: Option<&'static DepthPattern>,
}

impl FloorDepth {
    pub fn max(self) -> i32 {
        self.max
    }

    /// The depth at a floor top, through a per-column cache of the
    /// expression's evaluation.
    pub(crate) fn at_cached(
        self,
        seed: u32,
        [x, y, z]: [i32; 3],
        cache: &mut super::pattern::ColumnCache,
    ) -> i32 {
        let Some(pattern) = self.pattern else {
            return self.max;
        };
        let evaluation = cache.evaluation(&pattern.formula, [x, z], seed, || {
            Inputs([
                x as f64, y as f64, z as f64, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0,
            ])
        });
        let [depth] = evaluation.at::<1>(y as f64);
        (depth as i32).clamp(1, self.max)
    }

    pub(super) fn fingerprint(self) -> u64 {
        self.pattern.map_or(0, |p| p.fingerprint)
    }
}

impl RawDepth {
    pub(super) fn resolve(self) -> Result<FloorDepth, String> {
        let max = match &self {
            Self::Constant(v) => *v,
            Self::Pattern(p) => p.max,
        };
        if !(1..=MAX_FLOOR_DEPTH).contains(&max) {
            return Err(format!("floor depth must be in [1, {MAX_FLOOR_DEPTH}]"));
        }
        let pattern = match self {
            Self::Constant(_) => None,
            Self::Pattern(p) => {
                let bytes = serde_json::to_vec(&p).map_err(|e| e.to_string())?;
                let formula = Formula::compile(&p.bindings, &[p.extent])?;
                if formula.uses_any(&[3, 4, 5, 6, 7, 8, 9]) {
                    return Err("floor depth expressions may only use position inputs".into());
                }
                Some(&*Box::leak(Box::new(DepthPattern {
                    formula,
                    fingerprint: bytes.iter().fold(0xcbf29ce484222325_u64, |h, b| {
                        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
                    }),
                })))
            }
        };
        Ok(FloorDepth { max, pattern })
    }
}
