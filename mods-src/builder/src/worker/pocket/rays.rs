use crate::fx::{HashMap, HashSet};
use crate::geometry::{centre_of, feet_of, reaches, EYE};
use crate::worker::Task;

use super::graph::{Cell, Graph};
use super::recovery::Gate;

pub(super) struct Network {
    paths: HashMap<usize, Vec<Gate>>,
    below: HashSet<usize>,
}

impl Network {
    pub fn new(
        graph: &Graph,
        raw: &[Cell],
        targets: &[(usize, [i32; 3])],
        stances: &[[i32; 3]],
    ) -> Self {
        let bounds = graph.bounds;
        let column = |x: i32, z: i32| {
            let (dx, dz) = (x - bounds.min[0], z - bounds.min[2]);
            (dx >= 0 && dz >= 0 && (dx as usize) < bounds.size[0] && (dz as usize) < bounds.size[2])
                .then(|| dx as usize + bounds.size[0] * dz as usize)
        };
        let mut columns: Vec<Vec<i32>> = vec![Vec::new(); bounds.size[0] * bounds.size[2]];
        for stance in stances {
            if let Some(c) = column(stance[0], stance[2]) {
                columns[c].push(stance[1]);
            }
        }
        let highest = stances.iter().map(|s| s[1]).max().unwrap_or(i32::MIN);
        let air_sealed: HashSet<_> = graph.sealed.iter().map(|w| graph.work[*w].unit).collect();
        let mut network = Self {
            paths: HashMap::default(),
            below: HashSet::default(),
        };
        for (unit, target) in targets {
            let all_sides = air_sealed.contains(unit);
            if !all_sides && target[1] >= highest {
                continue;
            }
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
                        if !reaches(feet_of(stance), &[*target]) {
                            continue;
                        }
                        let Some((cost, path)) = path(graph, raw, stance, *target) else {
                            continue;
                        };
                        if y > target[1] && path.iter().any(|g| g.cell[1] > target[1]) {
                            network.below.insert(*unit);
                        }
                        if best.as_ref().is_none_or(|(least, _)| cost < *least) {
                            best = Some((cost, path));
                        }
                    }
                }
            }
            if let Some((_, path)) = best {
                network.paths.insert(*unit, path);
            }
        }
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
    for cell in crossed(eye, centre_of(target)) {
        if cell == target {
            continue;
        }
        let index = graph.bounds.index(cell)?;
        if raw[index].open || raw[index].sight {
            continue;
        }
        let (task, price) = raw[index].dig?;
        cost = cost.saturating_add(price);
        path.push(Gate {
            task,
            cell,
            serves: 1,
            cost: price,
            exposed: false,
        });
    }
    (!path.is_empty()).then_some((cost, path))
}

pub(super) fn crossed(from: [f64; 3], to: [f64; 3]) -> Vec<[i32; 3]> {
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
    let mut cells = vec![cell];
    while cell != end {
        let first = next.into_iter().reduce(f64::min).unwrap();
        for a in 0..3 {
            if next[a] <= first {
                cell[a] += step[a];
                next[a] += span[a];
            }
        }
        cells.push(cell);
    }
    cells
}

#[cfg(test)]
mod tests;
