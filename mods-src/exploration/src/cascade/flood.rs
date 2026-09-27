use std::collections::BTreeSet;

use mod_sdk::FxHashMap;

use super::basin::Basins;
use super::SIDES;

pub(super) type Reach = FxHashMap<[i32; 3], u8>;

pub(super) struct Geometry<'a> {
    pub(super) basins: &'a Basins,
    pub(super) wet: &'a BTreeSet<[i32; 3]>,
    pub(super) silt: &'a BTreeSet<[i32; 3]>,
    pub(super) cuts: &'a BTreeSet<[i32; 3]>,
    pub(super) bodies: &'a BTreeSet<[i32; 3]>,
}

#[derive(Copy, Clone, PartialEq)]
enum Class {
    Solid,
    Open,
    Source(usize),
    Unknown,
}

impl Geometry<'_> {
    fn class(&self, p: [i32; 3], terrain: &impl Fn([i32; 3]) -> Option<bool>) -> Class {
        if self.bodies.contains(&p) || self.silt.contains(&p) {
            return Class::Solid;
        }
        if self.cuts.contains(&p) {
            return Class::Open;
        }
        if self.wet.contains(&p) {
            return Class::Source(self.basins.cols[&(p[0], p[2])].0);
        }
        match terrain(p) {
            Some(true) => Class::Solid,
            Some(false) => Class::Open,
            None => Class::Unknown,
        }
    }

    fn sourcey(&self, p: [i32; 3]) -> bool {
        self.wet.contains(&p)
            || SIDES
                .iter()
                .filter(|&&(dx, dz)| self.wet.contains(&[p[0] + dx, p[1], p[2] + dz]))
                .count()
                >= 2
    }

    pub(super) fn flood(
        &self,
        terrain: &impl Fn([i32; 3]) -> Option<bool>,
    ) -> Result<Reach, &'static str> {
        let count = self.basins.counts();
        let mut best: Reach = FxHashMap::default();
        let mut stack: Vec<[i32; 3]> = Vec::new();
        for &p in self.wet {
            best.insert(p, 8);
            stack.push(p);
        }
        let mut delivered = vec![false; self.basins.pools.len()];
        delivered[0] = true;
        while let Some(p) = stack.pop() {
            let a = best[&p];
            let below = [p[0], p[1] - 1, p[2]];
            let mut sideways = false;
            match self.class(below, terrain) {
                Class::Unknown => return Err("water reaches past the probed domain"),
                Class::Open => {
                    if best.get(&below).copied().unwrap_or(0) < 8 {
                        best.insert(below, 8);
                        stack.push(below);
                    }
                }
                Class::Source(k) => {
                    if !self.wet.contains(&p) {
                        delivered[k] = true;
                    }
                    sideways = self.sourcey(p);
                }
                Class::Solid => sideways = true,
            }
            if sideways && a > 1 {
                for &(dx, dz) in &SIDES {
                    let q = [p[0] + dx, p[1], p[2] + dz];
                    match self.class(q, terrain) {
                        Class::Unknown => return Err("water reaches past the probed domain"),
                        Class::Solid => {}
                        class @ (Class::Open | Class::Source(_)) => {
                            if let Class::Source(k) = class {
                                if !self.wet.contains(&p) {
                                    delivered[k] = true;
                                }
                            }
                            if best.get(&q).copied().unwrap_or(0) < a - 1 {
                                best.insert(q, a - 1);
                                stack.push(q);
                            }
                        }
                    }
                }
            }
        }
        for (k, (&d, &c)) in delivered.iter().zip(count.iter()).enumerate() {
            if k > 0 && c > 0 && !d {
                return Err("a basin receives no fall");
            }
        }
        Ok(best)
    }
}

pub(super) fn reach_stays_in_extent(wet: &BTreeSet<[i32; 3]>, reach: &Reach) -> bool {
    let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
    for p in wet {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    reach
        .keys()
        .all(|p| (0..3).all(|k| p[k] >= lo[k] && p[k] <= hi[k]))
}
