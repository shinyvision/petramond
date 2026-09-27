use std::collections::{BTreeMap, BTreeSet};

use super::build::Built;
use super::flood::{Geometry, Reach};
use super::{Feature, Intruder, Kind};

impl Built {
    pub fn finish(
        &self,
        terrain: &impl Fn([i32; 3]) -> Option<bool>,
        intruders: &[Intruder],
    ) -> Feature {
        let mut standing: Vec<usize> = (0..intruders.len()).collect();
        standing.sort_by_key(|&k| intruders[k].key);
        let mut suppressed: Vec<[i32; 3]> = Vec::new();
        standing.retain(|&k| {
            let g = &intruders[k];
            if self.in_the_water(g) {
                suppressed.push(g.key);
                false
            } else {
                true
            }
        });
        let (reach, wet) = self.settle_giants(terrain, intruders, standing, &mut suppressed);
        self.assemble(&reach, &wet, suppressed)
    }

    fn in_the_water(&self, g: &Intruder) -> bool {
        let b = &self.basins;
        let floating = b
            .cols
            .get(&(g.root[0], g.root[2]))
            .is_some_and(|&(pi, _)| g.root[1] <= b.pools[pi] + 1);
        floating || g.solid.iter().any(|p| b.wet_at(p[0], p[2], p[1]))
    }

    fn in_the_way(&self, g: &Intruder) -> bool {
        g.solid
            .iter()
            .any(|p| self.reach.contains_key(p) || self.cuts.contains(p) || self.silt.contains(p))
            || self.basins.cols.contains_key(&(g.root[0], g.root[2]))
    }

    fn settle_giants(
        &self,
        terrain: &impl Fn([i32; 3]) -> Option<bool>,
        intruders: &[Intruder],
        mut standing: Vec<usize>,
        suppressed: &mut Vec<[i32; 3]>,
    ) -> (Reach, BTreeSet<[i32; 3]>) {
        loop {
            let mut bodies: BTreeSet<[i32; 3]> = BTreeSet::new();
            for &k in &standing {
                bodies.extend(intruders[k].solid.iter().copied());
            }
            let mut wet = self.basins.wet();
            wet.retain(|p| !bodies.contains(p));
            let proof = Geometry {
                basins: &self.basins,
                wet: &wet,
                silt: &self.silt,
                cuts: &self.cuts,
                bodies: &bodies,
            }
            .flood(terrain);
            match proof {
                Ok(reach) => return (reach, wet),
                Err(_) if standing.is_empty() => {
                    return (self.reach.clone(), self.basins.wet());
                }
                Err(_) => {
                    let blocker = standing
                        .iter()
                        .position(|&k| self.in_the_way(&intruders[k]));
                    match blocker {
                        Some(pos) => suppressed.push(intruders[standing.remove(pos)].key),
                        None => suppressed.extend(standing.drain(..).map(|k| intruders[k].key)),
                    }
                }
            }
        }
    }

    fn assemble(
        &self,
        reach: &Reach,
        wet: &BTreeSet<[i32; 3]>,
        suppressed: Vec<[i32; 3]>,
    ) -> Feature {
        let mut writes: BTreeMap<[i32; 3], Kind> = BTreeMap::new();
        for p in &self.silt {
            writes.insert(*p, Kind::Silt);
        }
        for p in &self.cuts {
            writes.insert(*p, Kind::Air);
        }
        for p in wet {
            writes.insert(*p, Kind::Water);
        }
        let mut reserves: BTreeSet<[i32; 3]> = BTreeSet::new();
        for p in reach.keys() {
            if !writes.contains_key(p) {
                reserves.insert(*p);
            }
        }
        for (p, k) in &writes {
            if matches!(k, Kind::Water | Kind::Air) {
                let up = [p[0], p[1] + 1, p[2]];
                if !writes.contains_key(&up) {
                    reserves.insert(up);
                }
            }
        }
        Feature {
            writes: writes.into_iter().collect(),
            reserves: reserves.into_iter().collect(),
            suppressed,
            #[cfg(test)]
            wet: reach.keys().copied().collect(),
        }
    }
}
