use super::{Expression, Inputs};
use crate::density::noise::ReferenceDoublePerlin;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseExpression {
    pub perlin: Perlin,
    pub at: [Expression; 3],
}

/// Two octave stacks with a named seed fork, independent of caller iteration order.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Perlin {
    pub salt: [u64; 2],
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

/// A seeded Perlin stack's identity: the seed and every parameter.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    seed: u32,
    salt: [u64; 2],
    octave: i32,
    amplitudes: [u64; 9],
    count: usize,
}

impl Perlin {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.amplitudes.is_empty()
            || self.amplitudes.len() > 9
            || self.amplitudes.iter().any(|a| !a.is_finite())
            || !(-12..=0).contains(&self.first_octave)
            || self.first_octave + self.amplitudes.len() as i32 > 1
        {
            return Err(
                "Perlin noise needs 1–9 finite amplitudes, with octaves within -12..=0".into(),
            );
        }
        Ok(())
    }

    pub(super) fn seeded(&self, seed: u32) -> Arc<ReferenceDoublePerlin> {
        let key = Key {
            seed,
            salt: self.salt,
            octave: self.first_octave,
            amplitudes: std::array::from_fn(|i| self.amplitudes.get(i).map_or(0, |v| v.to_bits())),
            count: self.amplitudes.len(),
        };
        crate::cache::process().noises.get_or_insert(key, || {
            Arc::new(ReferenceDoublePerlin::from_seed(
                seed.into(),
                self.salt,
                self.first_octave,
                &self.amplitudes,
            ))
        })
    }
}

impl super::Formula {
    pub fn column_seeded(&self, seed: u32, inputs: Inputs) -> super::Evaluation<'_> {
        let noises: super::Noises = self.noises.iter().map(|n| n.seeded(seed)).collect();
        let table = self.height_table(seed, &noises);
        let mut eval = super::Evaluation {
            formula: self,
            inputs,
            seed,
            values: [0.0; super::MAX_NODES],
            noises,
            table,
        };
        for &i in &self.column_ops {
            eval.values[i] =
                super::evaluate(&self.nodes[i], &eval.values, inputs, seed, &eval.noises);
        }
        eval
    }
}
