use crate::formula::{Expression, Formula, Inputs};
use petramond_world::block::Block;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPattern {
    #[serde(default)]
    bindings: Vec<(String, Expression)>,
    material: Expression,
    palette: Vec<String>,
    /// Sample the pattern's height-dependent noise this many blocks apart
    /// along a column and interpolate between: for noise far wider than
    /// the step, the same look for a fraction of the samples.
    #[serde(default)]
    lattice: Option<i32>,
}

#[derive(Debug)]
pub(crate) struct MaterialPattern {
    formula: Formula,
    palette: Box<[u16]>,
    pub fingerprint: u64,
    lattice: Option<i32>,
}

impl RawPattern {
    pub fn resolve(self) -> Result<&'static MaterialPattern, String> {
        if self.palette.is_empty() || self.palette.len() > 32 {
            return Err("lining pattern needs 1–32 materials".into());
        }
        if self.lattice.is_some_and(|step| !(2..=16).contains(&step)) {
            return Err("lining pattern lattice must be 2–16 blocks".into());
        }
        let bytes = serde_json::to_vec(&self).map_err(|e| e.to_string())?;
        let palette = self
            .palette
            .into_iter()
            .map(|name| {
                serde_json::from_value::<Block>(serde_json::Value::String(name))
                    .map(|b| b.id())
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let fingerprint = bytes.iter().fold(0xcbf29ce484222325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        Ok(Box::leak(Box::new(MaterialPattern {
            formula: Formula::compile(&self.bindings, &[self.material])?,
            palette: palette.into(),
            fingerprint,
            lattice: self.lattice,
        })))
    }
}

/// Column evaluations of position-only formulas, keyed by formula identity:
/// a column walk paints many cells from the same few patterns, and each
/// evaluation's column work is the expensive part.
#[derive(Default)]
pub(crate) struct ColumnCache {
    entries: Vec<(usize, [i32; 2], crate::formula::Evaluation<'static>)>,
}

impl ColumnCache {
    /// The formula's evaluation bound to `column`, rebinding one kept from
    /// another column rather than building a new register file.
    pub(crate) fn evaluation(
        &mut self,
        formula: &'static Formula,
        column: [i32; 2],
        seed: u32,
        inputs: impl FnOnce() -> Inputs,
    ) -> &mut crate::formula::Evaluation<'static> {
        let key = std::ptr::from_ref(formula) as usize;
        let i = match self.entries.iter().position(|(k, _, _)| *k == key) {
            Some(i) => {
                if self.entries[i].1 != column {
                    self.entries[i].1 = column;
                    self.entries[i].2.rebind(inputs());
                }
                i
            }
            None => {
                self.entries
                    .push((key, column, formula.column_seeded(seed, inputs())));
                self.entries.len() - 1
            }
        };
        &mut self.entries[i].2
    }
}

impl MaterialPattern {
    pub fn at(&self, seed: u32, [x, y, z]: [i32; 3]) -> u16 {
        self.column(seed, x, z, 0).at(y)
    }

    /// [`Self::at`] through a per-column cache of this pattern's evaluation.
    pub(crate) fn at_cached(
        &'static self,
        seed: u32,
        [x, y, z]: [i32; 3],
        cache: &mut ColumnCache,
    ) -> u16 {
        let evaluation = cache.evaluation(&self.formula, [x, z], seed, || Self::inputs(x, z, 0));
        let [index] = evaluation.at::<1>(y as f64);
        self.palette[index.max(0.0).min((self.palette.len() - 1) as f64) as usize]
    }

    /// A reusable scan of this pattern's formula.
    pub(crate) fn scanner(&'static self, seed: u32) -> crate::formula::Scan<'static> {
        self.formula.scan(seed)
    }

    /// The step the pattern asked its noise to be sampled on, if any.
    pub(crate) fn lattice(&self) -> Option<i32> {
        self.lattice
    }

    /// The palette materials selected by scanned indices.
    pub(crate) fn materials(&self, values: &[[f64; 1]], out: &mut Vec<u16>) {
        out.clear();
        out.extend(values.iter().map(|[index]| {
            self.palette[index.max(0.0).min((self.palette.len() - 1) as f64) as usize]
        }));
    }

    pub(crate) fn inputs(x: i32, z: i32, surface: i32) -> Inputs {
        Inputs([
            x as f64,
            0.0,
            z as f64,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            surface as f64,
            petramond_world::chunk::SEA_LEVEL as f64,
        ])
    }

    pub(crate) fn column(&self, seed: u32, x: i32, z: i32, surface: i32) -> MaterialColumn<'_> {
        MaterialColumn {
            palette: &self.palette,
            evaluation: self
                .formula
                .column_seeded(seed, Self::inputs(x, z, surface)),
        }
    }
}

pub(crate) struct MaterialColumn<'a> {
    palette: &'a [u16],
    evaluation: crate::formula::Evaluation<'a>,
}
impl MaterialColumn<'_> {
    pub(crate) fn at(&mut self, y: i32) -> u16 {
        let [index] = self.evaluation.at::<1>(y as f64);
        self.palette[index.max(0.0).min((self.palette.len() - 1) as f64) as usize]
    }
}
