//! Basin growth: classifying probed columns against a working surface,
//! growing the head basin and the chain of terraces below it, and adopting
//! the natural pits a basin encloses.

use mod_sdk::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, BTreeSet};

use super::site::Trace;
use super::{
    ADOPT_MAX, BED_BAND, GROW_DILATE, MAX_POOLS, MAX_STEP, MIN_POOLS, MIN_POOL_AREA, SIDES,
};

/// A floor column `(x, z)`.
pub(super) type Col = (i32, i32);

/// The basin chain: pool surfaces, links, and which pool owns each column.
pub(super) struct Basins {
    /// Water-surface row of each pool (index = pool id). A pool emptied by
    /// the seal retreat keeps its slot with no columns.
    pub(super) pools: Vec<i32>,
    /// Which pool spills into pool `j` (`from[0]` is unused).
    pub(super) from: Vec<usize>,
    /// Column -> (pool, bed row). Water occupies `bed+1..=surface`.
    pub(super) cols: BTreeMap<Col, (usize, i32)>,
}

impl Basins {
    /// Is `(x, y, z)` water as the chain stands?
    pub(super) fn wet_at(&self, x: i32, z: i32, y: i32) -> bool {
        self.cols
            .get(&(x, z))
            .is_some_and(|&(pi, bed)| y > bed && y <= self.pools[pi])
    }

    /// The pool that owns a column, if any.
    pub(super) fn pool_of(&self, c: Col) -> Option<usize> {
        self.cols.get(&c).map(|&(pi, _)| pi)
    }

    /// Columns each pool still holds.
    pub(super) fn counts(&self) -> Vec<usize> {
        let mut count = vec![0usize; self.pools.len()];
        for &(pi, _) in self.cols.values() {
            count[pi] += 1;
        }
        count
    }

    /// The wet set: water from over the bed up to the surface.
    pub(super) fn wet(&self) -> BTreeSet<[i32; 3]> {
        let mut wet = BTreeSet::new();
        for (&(x, z), &(pi, bed)) in &self.cols {
            for y in bed + 1..=self.pools[pi] {
                wet.insert([x, y, z]);
            }
        }
        wet
    }
}

/// A column's classification at a working surface `s`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(super) enum Mem {
    /// Basin member; water `bed+1..=s`, bed one under the floor.
    In(i32),
    /// Floor above the surface: natural shore.
    Rock,
    /// Floor a full step below: a candidate seed for the NEXT basin (its
    /// would-be surface carried along), or too deep to use.
    Step(Option<i32>),
    /// Past the probe or the growth band.
    Off,
}

/// The probed window of one trace, and the columns a basin may grow into.
pub(super) struct Survey<'t, T> {
    /// Probe column -> its inclusive row window.
    window: FxHashMap<Col, (i32, i32)>,
    /// Columns within `GROW_DILATE` of a traced sample.
    growable: FxHashSet<Col>,
    terrain: &'t T,
}

impl<'t, T: Fn([i32; 3]) -> Option<bool>> Survey<'t, T> {
    pub(super) fn new(trace: &Trace, terrain: &'t T) -> Survey<'t, T> {
        let window: FxHashMap<Col, (i32, i32)> = trace.probe_cols().into_iter().collect();
        let mut growable = FxHashSet::default();
        for &(sx, sz, _) in &trace.samples {
            for x in sx - GROW_DILATE..=sx + GROW_DILATE {
                for z in sz - GROW_DILATE..=sz + GROW_DILATE {
                    growable.insert((x, z));
                }
            }
        }
        Survey {
            window,
            growable,
            terrain,
        }
    }

    fn in_window(&self, x: i32, z: i32, y: i32) -> bool {
        self.window
            .get(&(x, z))
            .is_some_and(|&(lo, hi)| y >= lo && y <= hi)
    }

    /// World box the probes cover, for the caller to gather giants against.
    pub(super) fn domain(&self) -> ([i32; 3], [i32; 3]) {
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for (&(x, z), &(l, h)) in self.window.iter() {
            lo = [lo[0].min(x), lo[1].min(l), lo[2].min(z)];
            hi = [hi[0].max(x), hi[1].max(h), hi[2].max(z)];
        }
        (lo, hi)
    }

    pub(super) fn classify(&self, x: i32, z: i32, s: i32) -> Mem {
        let terrain = self.terrain;
        if !self.growable.contains(&(x, z)) || !self.in_window(x, z, s) {
            return Mem::Off;
        }
        match terrain([x, s, z]) {
            None => return Mem::Off,
            Some(true) => return Mem::Rock,
            Some(false) => {}
        }
        let mut b = s - 1;
        loop {
            match terrain([x, b, z]) {
                None => return Mem::Off,
                Some(true) => {
                    let depth = s - b;
                    return if depth <= BED_BAND + 1 {
                        Mem::In(b)
                    } else if depth <= MAX_STEP + 1 {
                        Mem::Step(Some(b + 1))
                    } else {
                        Mem::Step(None)
                    };
                }
                Some(false) => {
                    if s - b > MAX_STEP {
                        return Mem::Step(None);
                    }
                    b -= 1;
                }
            }
        }
    }

    /// Region-grow one basin at surface `s` from `seed_col`, over columns no
    /// other basin owns. Returns member column -> bed row.
    fn grow(
        &self,
        s: i32,
        seed_col: Col,
        owned: &BTreeMap<Col, (usize, i32)>,
    ) -> BTreeMap<Col, i32> {
        let mut members: BTreeMap<Col, i32> = BTreeMap::new();
        let mut seen: BTreeSet<Col> = BTreeSet::new();
        let mut frontier: BTreeSet<Col> = BTreeSet::new();
        frontier.insert(seed_col);
        seen.insert(seed_col);
        while let Some(c) = frontier.pop_first() {
            if owned.contains_key(&c) {
                continue;
            }
            let Mem::In(bed) = self.classify(c.0, c.1, s) else {
                continue;
            };
            members.insert(c, bed);
            for (dx, dz) in SIDES {
                let nc = (c.0 + dx, c.1 + dz);
                if seen.insert(nc) {
                    frontier.insert(nc);
                }
            }
        }
        members
    }

    /// The head basin behind the trace's anchor, then terrace after terrace
    /// down the fall line until the chain is full or finds no next step.
    pub(super) fn grow_chain(&self, trace: &Trace) -> Result<Basins, &'static str> {
        let mut basins = Basins {
            pools: Vec::new(),
            from: Vec::new(),
            cols: BTreeMap::new(),
        };
        let head = self.grow(trace.s0, trace.anchor, &basins.cols);
        if head.len() < MIN_POOL_AREA {
            return Err("head basin found no terrace");
        }
        for (c, bed) in head {
            basins.cols.insert(c, (0, bed));
        }
        basins.pools.push(trace.s0);
        basins.from.push(0);
        let mut banned: BTreeSet<Col> = BTreeSet::new();
        while basins.pools.len() < MAX_POOLS {
            let Some((f, seed_col, pi)) = self.next_seed(&basins, &banned) else {
                break;
            };
            let pool = self.grow(f, seed_col, &basins.cols);
            if pool.len() < MIN_POOL_AREA {
                banned.insert(seed_col);
                continue;
            }
            let id = basins.pools.len();
            for (c, bed) in pool {
                basins.cols.insert(c, (id, bed));
            }
            basins.pools.push(f);
            basins.from.push(pi);
        }
        if basins.pools.len() < MIN_POOLS {
            return Err("no descent: the floor offers no second terrace");
        }
        Ok(basins)
    }

    /// The next basin seeds from the highest floor a full step below any
    /// existing basin's surface, just past its edge — the fall line, or the
    /// contour continuing to roll downward. `(surface, seed column, source
    /// pool)`.
    fn next_seed(&self, basins: &Basins, banned: &BTreeSet<Col>) -> Option<(i32, Col, usize)> {
        let mut best: Option<(i32, Col, usize)> = None;
        for (&(x, z), &(pi, _)) in &basins.cols {
            for (dx, dz) in SIDES {
                let nc = (x + dx, z + dz);
                if basins.cols.contains_key(&nc) || banned.contains(&nc) {
                    continue;
                }
                if let Mem::Step(Some(f)) = self.classify(nc.0, nc.1, basins.pools[pi]) {
                    let cand = (f, nc, pi);
                    if best.is_none_or(|b| {
                        (cand.0, std::cmp::Reverse((cand.1, cand.2)))
                            > (b.0, std::cmp::Reverse((b.1, b.2)))
                    }) {
                        best = Some(cand);
                    }
                }
            }
        }
        best
    }
}

/// A too-deep column ENCLOSED by one basin is a natural pit: it joins as a
/// deep spot, adopted down to `ADOPT_MAX` and floored with a suspended silt
/// plate past that. Open-sided deep ground is the downhill break and stays
/// out.
pub(super) fn adopt_pits(basins: &mut Basins, terrain: &impl Fn([i32; 3]) -> Option<bool>) {
    let (mut x0, mut x1, mut z0, mut z1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, z) in basins.cols.keys() {
        x0 = x0.min(x);
        x1 = x1.max(x);
        z0 = z0.min(z);
        z1 = z1.max(z);
    }
    let outside = outside_of(&basins.cols, (x0, x1, z0, z1));
    let mut adopt: Vec<(Col, (usize, i32))> = Vec::new();
    for x in x0..=x1 {
        for z in z0..=z1 {
            let c = (x, z);
            if basins.cols.contains_key(&c) || outside.contains(&c) {
                continue;
            }
            // The pit joins the pool that surrounds it: any cardinal member
            // neighbour names it (enclosed, so one exists).
            let Some(&(pi, _)) = SIDES
                .iter()
                .find_map(|&(dx, dz)| basins.cols.get(&(x + dx, z + dz)))
            else {
                continue;
            };
            let s = basins.pools[pi];
            if terrain([x, s, z]) != Some(false) {
                continue; // a rock island stays an island
            }
            let mut b = s - 1;
            while s - b < ADOPT_MAX && terrain([x, b, z]) == Some(false) {
                b -= 1;
            }
            if terrain([x, b, z]).is_none() {
                continue;
            }
            adopt.push((c, (pi, b)));
        }
    }
    for (c, v) in adopt {
        basins.cols.insert(c, v);
    }
}

/// Flood the non-member columns from the rim of the members' bounding box
/// (grown by one); whatever it cannot reach is enclosed.
fn outside_of(
    cols: &BTreeMap<Col, (usize, i32)>,
    (x0, x1, z0, z1): (i32, i32, i32, i32),
) -> BTreeSet<Col> {
    let mut outside: BTreeSet<Col> = BTreeSet::new();
    let mut stack: Vec<Col> = Vec::new();
    for x in x0 - 1..=x1 + 1 {
        for z in [z0 - 1, z1 + 1] {
            stack.push((x, z));
        }
    }
    for z in z0 - 1..=z1 + 1 {
        for x in [x0 - 1, x1 + 1] {
            stack.push((x, z));
        }
    }
    while let Some(c) = stack.pop() {
        if c.0 < x0 - 1 || c.0 > x1 + 1 || c.1 < z0 - 1 || c.1 > z1 + 1 {
            continue;
        }
        if cols.contains_key(&c) || !outside.insert(c) {
            continue;
        }
        for (dx, dz) in SIDES {
            stack.push((c.0 + dx, c.1 + dz));
        }
    }
    outside
}
