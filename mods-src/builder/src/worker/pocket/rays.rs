use crate::fx::{HashMap, HashSet};
use crate::geometry::{centre_of, feet_of, reaches, EYE};
use crate::worker::Task;

use super::graph::{Cell, Graph};
use super::grid::Bounds;
use super::recovery::Gate;

/// Changed cells beyond this many make every target's answer worth working out again.
const CHANGES_MAX: usize = 4096;

pub(super) struct Network {
    paths: HashMap<usize, Vec<Gate>>,
    below: HashSet<usize>,
}

/// Each target's answer from the last network, and the cells and stances it was worked out from.
#[derive(Default)]
pub(super) struct Remembered {
    bounds: Option<Bounds>,
    raw: Vec<Cell>,
    stances: Vec<bool>,
    answers: HashMap<usize, Answer>,
}

struct Answer {
    target: [i32; 3],
    all_sides: bool,
    skipped: bool,
    path: Option<Vec<Gate>>,
    below: bool,
}

impl Remembered {
    /// Cells whose target may answer differently: a target's rays and stances stay within four
    /// cells across and five up or down of it. `None` when every target may.
    fn unsettled(&self, bounds: Bounds, raw: &[Cell], stances: &[bool]) -> Option<Vec<bool>> {
        if self.bounds != Some(bounds) || self.raw.len() != raw.len() {
            return None;
        }
        let changed: Vec<usize> = (0..raw.len())
            .filter(|&i| raw[i] != self.raw[i] || stances[i] != self.stances[i])
            .collect();
        if changed.len() > CHANGES_MAX {
            return None;
        }
        let mut unsettled = vec![false; raw.len()];
        for c in changed {
            let cell = bounds.cell(c);
            for dx in -4..=4 {
                for dy in -5..=5 {
                    for dz in -4..=4 {
                        if let Some(i) = bounds.index([cell[0] + dx, cell[1] + dy, cell[2] + dz]) {
                            unsettled[i] = true;
                        }
                    }
                }
            }
        }
        Some(unsettled)
    }
}

impl Network {
    #[cfg(test)]
    pub fn new(
        graph: &Graph,
        raw: &[Cell],
        targets: &[(usize, [i32; 3])],
        stances: &[[i32; 3]],
    ) -> Self {
        Self::remembering(graph, raw, targets, stances, &mut Remembered::default())
    }

    /// The network, working out again only the targets whose cells or stances changed since `remembered`.
    pub fn remembering(
        graph: &Graph,
        raw: &[Cell],
        targets: &[(usize, [i32; 3])],
        stances: &[[i32; 3]],
        remembered: &mut Remembered,
    ) -> Self {
        let bounds = graph.bounds;
        let column = |x: i32, z: i32| {
            let (dx, dz) = (x - bounds.min[0], z - bounds.min[2]);
            (dx >= 0 && dz >= 0 && (dx as usize) < bounds.size[0] && (dz as usize) < bounds.size[2])
                .then(|| dx as usize + bounds.size[0] * dz as usize)
        };
        let mut columns: Vec<Vec<i32>> = vec![Vec::new(); bounds.size[0] * bounds.size[2]];
        let mut standing = vec![false; raw.len()];
        for stance in stances {
            if let Some(c) = column(stance[0], stance[2]) {
                columns[c].push(stance[1]);
            }
            if let Some(i) = bounds.index(*stance) {
                standing[i] = true;
            }
        }
        // Lowest first, so equally cheap ways in are chosen the same way every time.
        for ys in &mut columns {
            ys.sort_unstable();
            ys.dedup();
        }
        let highest = stances.iter().map(|s| s[1]).max().unwrap_or(i32::MIN);
        let air_sealed: HashSet<_> = graph.sealed.iter().map(|w| graph.work[*w].unit).collect();
        let unsettled = remembered.unsettled(bounds, raw, &standing);
        let mut previous = std::mem::take(&mut remembered.answers);
        let mut network = Self {
            paths: HashMap::default(),
            below: HashSet::default(),
        };
        for (unit, target) in targets {
            let all_sides = air_sealed.contains(unit);
            let skipped = !all_sides && target[1] >= highest;
            let settled = unsettled
                .as_ref()
                .is_some_and(|unsettled| bounds.index(*target).is_some_and(|i| !unsettled[i]));
            let answer = match previous.remove(unit) {
                Some(answer)
                    if settled
                        && answer.target == *target
                        && answer.all_sides == all_sides
                        && answer.skipped == skipped =>
                {
                    answer
                }
                _ => {
                    let mut answer = Answer {
                        target: *target,
                        all_sides,
                        skipped,
                        path: None,
                        below: false,
                    };
                    if !skipped {
                        let mut best: Option<(u32, Vec<Gate>)> = None;
                        for x in target[0] - 4..=target[0] + 4 {
                            for z in target[2] - 4..=target[2] + 4 {
                                for &y in column(x, z).map_or(&[][..], |c| &columns[c]) {
                                    if !(target[1] - 5..=target[1] + 3).contains(&y)
                                        || (!all_sides && y <= target[1])
                                    {
                                        continue;
                                    }
                                    let stance = [x, y, z];
                                    if !reaches_from(x - target[0], y - target[1], z - target[2]) {
                                        continue;
                                    }
                                    let Some((cost, path)) = path(graph, raw, stance, *target)
                                    else {
                                        continue;
                                    };
                                    if y > target[1] && path.iter().any(|g| g.cell[1] > target[1]) {
                                        answer.below = true;
                                    }
                                    if best.as_ref().is_none_or(|(least, _)| cost < *least) {
                                        best = Some((cost, path));
                                    }
                                }
                            }
                        }
                        answer.path = best.map(|(_, path)| path);
                    }
                    answer
                }
            };
            if answer.below {
                network.below.insert(*unit);
            }
            if let Some(path) = &answer.path {
                network.paths.insert(*unit, path.clone());
            }
            remembered.answers.insert(*unit, answer);
        }
        remembered.bounds = Some(bounds);
        remembered.raw = raw.to_vec();
        remembered.stances = standing;
        network
    }

    pub fn below(&self) -> impl Iterator<Item = usize> + '_ {
        self.below.iter().copied()
    }

    pub fn opens(&self, unit: usize) -> bool {
        self.paths.get(&unit).is_some_and(|path| !path.is_empty())
    }

    pub fn openings(&self, units: &[usize]) -> Vec<Gate> {
        let mut gates: HashMap<Task, Gate> = HashMap::default();
        for unit in units {
            let mut counted = HashSet::default();
            for gate in self.paths.get(unit).into_iter().flatten() {
                if counted.insert(gate.task) {
                    gates
                        .entry(gate.task)
                        .and_modify(|g| g.serves += 1)
                        .or_insert(*gate);
                }
            }
        }
        gates.into_values().collect()
    }
}

/// Whether a stance this far from a target reaches it; reach depends on the offset alone.
fn reaches_from(dx: i32, dy: i32, dz: i32) -> bool {
    static TABLE: std::sync::OnceLock<Vec<bool>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = Vec::new();
        for dx in -4..=4 {
            for dy in -5..=3 {
                for dz in -4..=4 {
                    table.push(reaches(feet_of([dx, dy, dz]), &[[0, 0, 0]]));
                }
            }
        }
        table
    });
    (-4..=4).contains(&dx)
        && (-5..=3).contains(&dy)
        && (-4..=4).contains(&dz)
        && table[((dx + 4) * 9 + dy + 5) as usize * 9 + (dz + 4) as usize]
}

fn path(
    graph: &Graph,
    raw: &[Cell],
    stance: [i32; 3],
    target: [i32; 3],
) -> Option<(u32, Vec<Gate>)> {
    let mut eye = feet_of(stance);
    eye[1] += EYE;
    let mut path = Vec::new();
    let mut cost = 0_u32;
    let mut through = true;
    walk(eye, centre_of(target), |cell| {
        if cell == target {
            return true;
        }
        let Some(index) = graph.bounds.index(cell) else {
            through = false;
            return false;
        };
        if raw[index].open || raw[index].sight {
            return true;
        }
        let Some((task, price)) = raw[index].dig else {
            through = false;
            return false;
        };
        cost = cost.saturating_add(price);
        path.push(Gate {
            task,
            cell,
            serves: 1,
            cost: price,
            exposed: false,
        });
        true
    });
    (through && !path.is_empty()).then_some((cost, path))
}

/// Visits the cells a segment crosses in order, until `visit` answers false.
pub(super) fn walk(from: [f64; 3], to: [f64; 3], mut visit: impl FnMut([i32; 3]) -> bool) {
    let mut cell = from.map(|n| n.floor() as i32);
    let end = to.map(|n| n.floor() as i32);
    let delta: [f64; 3] = std::array::from_fn(|a| to[a] - from[a]);
    let step = delta.map(|n| {
        if n > 0.0 {
            1
        } else if n < 0.0 {
            -1
        } else {
            0
        }
    });
    let span = delta.map(|n| {
        if n == 0.0 {
            f64::INFINITY
        } else {
            n.abs().recip()
        }
    });
    let mut next: [f64; 3] = std::array::from_fn(|a| {
        if step[a] == 0 {
            f64::INFINITY
        } else {
            let edge = cell[a] + i32::from(step[a] > 0);
            (f64::from(edge) - from[a]) / delta[a]
        }
    });
    if !visit(cell) {
        return;
    }
    while cell != end {
        let first = next.into_iter().reduce(f64::min).unwrap();
        for a in 0..3 {
            if next[a] <= first {
                cell[a] += step[a];
                next[a] += span[a];
            }
        }
        if !visit(cell) {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
