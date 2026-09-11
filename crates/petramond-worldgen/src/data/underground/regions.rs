//! Spatial admission before nearest-climate fallback.

use serde::{Deserialize, Serialize};

use super::{ClimatePoint, ClimateRange, IdSet, UndergroundBiomes};
use crate::density::noise::Xoroshiro;
use petramond_math::noise::{cellular2, simplex2_permutation};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegionShape {
    radius: f64,
    separation: f64,
    jitter: f64,
    warp: f64,
    warp_seeds: [u64; 2],
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRegion {
    shape: RegionShape,
    bucket: [u16; 2],
    #[serde(default)]
    min_origin_distance: f64,
}

impl RawRegion {
    pub(super) fn validate(&self) -> Result<(), String> {
        let s = &self.shape;
        if !s.radius.is_finite()
            || !(4.0..=32768.0).contains(&s.radius)
            || !s.separation.is_finite()
            || !(4.0..=131072.0).contains(&s.separation)
            || !(0.0..=1.0).contains(&s.jitter)
            || !(0.0..=1.0).contains(&s.warp)
        {
            return Err("'region.shape' requires finite radius [4, 32768], separation [4, 131072], jitter and warp [0, 1]".into());
        }
        if self.bucket[1] == 0 || self.bucket[0] >= self.bucket[1] {
            return Err("'region.bucket' must be [index, count] with index < count".into());
        }
        if !self.min_origin_distance.is_finite() || self.min_origin_distance < 0.0 {
            return Err("'region.min_origin_distance' must be finite and nonnegative".into());
        }
        Ok(())
    }
}

struct Selector {
    id: u8,
    bucket: [u16; 2],
    origin_distance_squared: f64,
    climate: ClimateRange,
}

pub(crate) struct RegionGroup {
    shape: RegionShape,
    permutations: [[u8; 256]; 2],
    selectors: Vec<Selector>,
}

pub(super) fn compile(rows: &[super::UndergroundBiomeDef]) -> Box<[RegionGroup]> {
    let mut groups: Vec<RegionGroup> = Vec::new();
    let mut order: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.region.is_some())
        .collect();
    order.sort_by_key(|(_, r)| r.name);
    for (id, row) in order {
        let region = row.region.as_ref().expect("filtered regional selector");
        let index = groups
            .iter()
            .position(|g| g.shape == region.shape)
            .unwrap_or_else(|| {
                groups.push(RegionGroup {
                    shape: region.shape.clone(),
                    permutations: region.shape.warp_seeds.map(permutation),
                    selectors: Vec::new(),
                });
                groups.len() - 1
            });
        groups[index].selectors.push(Selector {
            id: id as u8,
            bucket: region.bucket,
            origin_distance_squared: region.min_origin_distance.powi(2),
            climate: row.climate.expect("validated selector"),
        });
    }
    groups.into_boxed_slice()
}

fn permutation(seed: u64) -> [u8; 256] {
    let mut random = Xoroshiro::new(seed);
    // Origins are part of the random stream even when sampling without them.
    for _ in 0..3 {
        random.next_double();
    }
    let mut out = std::array::from_fn(|i| i as u8);
    for i in 0..256 {
        let j = i + random.next_int((256 - i) as u32) as usize;
        out.swap(i, j);
    }
    out
}

impl RegionGroup {
    pub(crate) fn candidates(
        &self,
        seed: u32,
        quart: [i32; 2],
        climate_at: impl Fn([i32; 2]) -> ClimatePoint,
        out: &mut IdSet,
    ) {
        let spacing = (self.shape.radius + self.shape.separation) * 0.25;
        let point = quart.map(|v| f64::from(v) / spacing);
        let warped = std::array::from_fn(|a| {
            point[a]
                + self.shape.warp * simplex2_permutation(&self.permutations[a], point[0], point[1])
        });
        let cell = cellular2(seed, warped, self.shape.jitter);
        if cell.distance >= self.shape.radius * 0.25 / spacing {
            return;
        }
        let label = (f64::from(cell.hash) / 2147483648.0 + 1.0) * 0.5;
        let distance = quart
            .into_iter()
            .map(|v| (f64::from(v) * 4.0).powi(2))
            .sum::<f64>();
        let center = cell.center.map(|v| (v * spacing).floor() as i32 * 4);
        let mut climate = None;
        for selector in &self.selectors {
            if (label * f64::from(selector.bucket[1])) as u16 != selector.bucket[0]
                || distance < selector.origin_distance_squared
            {
                continue;
            }
            let values = *climate.get_or_insert_with(|| climate_at(center));
            if selector.climate.axes()[..5]
                .iter()
                .zip(values)
                .all(|(&range, value)| contains(range, value))
            {
                out.insert(selector.id);
            }
        }
    }
}

impl UndergroundBiomes {
    pub(crate) fn region_id(&self, candidates: IdSet, height: f64, y: i32) -> Option<u8> {
        let depth = (height - f64::from(y.div_euclid(4) * 4)) / 128.0;
        candidates
            .iter()
            .filter(|&id| {
                let row = &self.catalog.rows()[id as usize];
                (row.whole_column || (row.y.0..=row.y.1).contains(&y))
                    && contains(row.climate.expect("regional climate").axes()[5], depth)
            })
            .min_by_key(|&id| self.catalog.rows()[id as usize].name)
    }

    pub(crate) fn region_ids_in(
        &self,
        candidates: IdSet,
        height: f64,
        y: [i32; 2],
        out: &mut IdSet,
    ) {
        let depth = y.map(|v| quantized((height - f64::from(v.div_euclid(4) * 4)) / 128.0));
        for id in candidates.iter() {
            let row = &self.catalog.rows()[id as usize];
            let range = row.climate.expect("regional climate").axes()[5];
            if row.y.0 <= y[1]
                && row.y.1 >= y[0]
                && range[0] as f32 <= depth[0]
                && range[1] as f32 >= depth[1]
            {
                out.insert(id);
            }
        }
    }

    pub(crate) fn shell(&self, id: u8) -> f64 {
        self.catalog.rows()[id as usize].shell
    }
}

fn quantized(value: f64) -> f32 {
    ((value as f32 * 10000.0) as i64) as f32 / 10000.0
}

fn contains([lo, hi]: [f64; 2], value: f64) -> bool {
    (lo as f32..=hi as f32).contains(&quantized(value))
}

#[cfg(test)]
mod tests;
