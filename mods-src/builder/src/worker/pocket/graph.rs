use crate::fx::HashMap;
use crate::worker::Task;

use super::grid::Bounds;

type Check = (Option<usize>, Vec<usize>, Vec<[i32; 3]>);

#[derive(Clone, Copy, Default)]
pub(super) struct Cell {
    pub open: bool,
    pub sight: bool,
    pub dig: Option<(Task, u32)>,
    pub head: Option<(Task, u32)>,
}

pub(super) struct Work {
    pub unit: usize,
    pub contacts: Vec<usize>,
}

pub(super) struct Graph {
    pub bounds: Bounds,
    pub cells: Vec<Cell>,
    pub work: Vec<Work>,
    pub root: usize,
    pub reached: Vec<bool>,
    pub sealed: Vec<usize>,
    pub components: Vec<usize>,
    pub component_count: usize,
    pub aims: HashMap<usize, super::sight::Aims>,
    pub sight_open: Vec<bool>,
    pub ground: crate::fx::HashSet<usize>,
    pub supports: HashMap<usize, usize>,
    physical: HashMap<usize, Vec<usize>>,
    cuts: Vec<bool>,
    checks: HashMap<Check, Option<usize>>,
}

impl Graph {
    pub fn new(bounds: Bounds, cells: Vec<Cell>, work: Vec<Work>, root: usize) -> Self {
        let mut graph = Self {
            bounds,
            reached: vec![false; cells.len()],
            components: vec![usize::MAX; cells.len()],
            cuts: vec![false; cells.len()],
            component_count: 0,
            sight_open: cells.iter().map(|c| c.open || c.sight).collect(),
            cells,
            work,
            root,
            sealed: Vec::new(),
            checks: HashMap::default(),
            aims: HashMap::default(),
            ground: Default::default(),
            supports: HashMap::default(),
            physical: HashMap::default(),
        };
        graph.components();
        let root_component = graph.components[root];
        for (i, reached) in graph.reached.iter_mut().enumerate() {
            *reached = root_component != usize::MAX && graph.components[i] == root_component;
        }
        graph.sealed = graph
            .work
            .iter()
            .enumerate()
            .filter(|(_, w)| !graph.contacts(w).iter().any(|c| graph.reached[*c]))
            .map(|(i, _)| i)
            .collect();
        graph.articulations();
        graph
    }

    #[cfg(test)]
    pub fn set_aim(&mut self, unit: usize, node: usize, aim: super::sight::Aim) {
        let points = self.aims.entry(unit).or_default();
        let points = std::sync::Arc::make_mut(points);
        points.push((node, aim));
        points.sort_unstable_by_key(|(node, _)| *node);
    }

    fn aim(&self, unit: usize, node: usize) -> Option<&super::sight::Aim> {
        let points = self.aims.get(&unit)?;
        let i = points.binary_search_by_key(&node, |(node, _)| *node).ok()?;
        Some(&points[i].1)
    }

    pub fn physical(&mut self, contacts: HashMap<usize, Vec<usize>>) {
        self.physical = contacts;
        self.sealed = self
            .work
            .iter()
            .enumerate()
            .filter(|(_, w)| !self.contacts(w).iter().any(|c| self.reached[*c]))
            .map(|(i, _)| i)
            .collect();
        self.checks.clear();
    }

    pub fn verified(&self, unit: usize, cell: [i32; 3]) -> bool {
        self.physical.get(&unit).is_none_or(|contacts| {
            self.bounds
                .index(cell)
                .is_some_and(|i| contacts.contains(&i))
        })
    }

    pub fn needs_ground(&self) -> bool {
        !self.physical.is_empty()
    }

    pub fn seals_ground(
        &self,
        except: Option<usize>,
        body: &[[i32; 3]],
        walls: &[[i32; 3]],
        reachable: impl Fn([i32; 3]) -> bool,
    ) -> Option<usize> {
        self.work
            .iter()
            .find(|w| {
                Some(w.unit) != except
                    && self.physical.contains_key(&w.unit)
                    && self.contacts(w).iter().any(|c| self.reached[*c])
                    && !self.contacts(w).iter().any(|c| {
                        let cell = self.bounds.cell(*c);
                        let supported = self.supports.get(c).is_some_and(|base| {
                            let base = self.bounds.cell(*base);
                            reachable(base)
                                && !walls.iter().any(|wall| {
                                    wall[0] == cell[0]
                                        && wall[2] == cell[2]
                                        && (base[1]..=cell[1] + 1).contains(&wall[1])
                                })
                        });
                        !body.contains(&cell)
                            && (reachable(cell) || supported)
                            && self.aim(w.unit, *c).is_none_or(|aim| {
                                !super::sight::blocked(
                                    cell,
                                    *aim,
                                    walls,
                                    self.bounds,
                                    &self.sight_open,
                                )
                            })
                    })
            })
            .map(|w| w.unit)
    }

    fn contacts<'a>(&'a self, work: &'a Work) -> &'a [usize] {
        self.physical
            .get(&work.unit)
            .map_or(&work.contacts, |c| c.as_slice())
    }

    fn components(&mut self) {
        let mut queue = Vec::new();
        for start in 0..self.cells.len() {
            if !self.cells[start].open || self.components[start] != usize::MAX {
                continue;
            }
            queue.clear();
            queue.push(start);
            self.components[start] = self.component_count;
            let mut at = 0;
            while at < queue.len() {
                let cell = queue[at];
                at += 1;
                for next in self.bounds.neighbours(cell).into_iter().flatten() {
                    if self.cells[next].open && self.components[next] == usize::MAX {
                        self.components[next] = self.component_count;
                        queue.push(next);
                    }
                }
            }
            self.component_count += 1;
        }
    }

    fn articulations(&mut self) {
        if !self.reached[self.root] {
            return;
        }
        let mut order = vec![0usize; self.cells.len()];
        let mut low = vec![0usize; self.cells.len()];
        let mut parent = vec![usize::MAX; self.cells.len()];
        let mut stack = vec![(self.root, 0usize)];
        let mut next_order = 1;
        order[self.root] = next_order;
        low[self.root] = next_order;
        while let Some((cell, next)) = stack.last_mut() {
            let cell = *cell;
            if *next < 6 {
                let neighbour = self.bounds.neighbours(cell)[*next];
                *next += 1;
                let Some(n) = neighbour.filter(|n| self.reached[*n]) else {
                    continue;
                };
                if order[n] == 0 {
                    next_order += 1;
                    order[n] = next_order;
                    low[n] = next_order;
                    parent[n] = cell;
                    stack.push((n, 0));
                } else if parent[cell] != n {
                    low[cell] = low[cell].min(order[n]);
                }
            } else {
                stack.pop();
                let p = parent[cell];
                if p != usize::MAX {
                    low[p] = low[p].min(low[cell]);
                    if low[cell] >= order[p] {
                        self.cuts[p] = true;
                    }
                }
            }
        }
        // Removing the home cell always removes the graph's source.
        self.cuts[self.root] = true;
    }

    #[cfg(test)]
    pub fn seals(&mut self, except: Option<usize>, cells: &[[i32; 3]]) -> Option<usize> {
        self.seals_sight(except, cells, cells)
    }

    pub fn seals_sight(
        &mut self,
        except: Option<usize>,
        cells: &[[i32; 3]],
        walls: &[[i32; 3]],
    ) -> Option<usize> {
        let mut blocked: Vec<usize> = cells
            .iter()
            .filter_map(|c| self.bounds.index(*c))
            .filter(|i| self.reached[*i])
            .collect();
        blocked.sort_unstable();
        blocked.dedup();
        if blocked.is_empty() && self.aims.is_empty() {
            return None;
        }
        let key = (except, blocked, walls.to_vec());
        if let Some(answer) = self.checks.get(&key) {
            return *answer;
        }
        let blocked = &key.1;
        let mut after;
        let reached = if blocked.is_empty() || (blocked.len() == 1 && !self.cuts[blocked[0]]) {
            &self.reached
        } else {
            after = vec![false; self.cells.len()];
            let mut queue = Vec::new();
            if !blocked.contains(&self.root) {
                queue.push(self.root);
                after[self.root] = true;
            }
            let mut at = 0;
            while at < queue.len() {
                let cell = queue[at];
                at += 1;
                for next in self.bounds.neighbours(cell).into_iter().flatten() {
                    if self.reached[next] && !after[next] && !blocked.contains(&next) {
                        after[next] = true;
                        queue.push(next);
                    }
                }
            }
            &after
        };
        let sealed = self.work.iter().find(|w| {
            Some(w.unit) != except
                && self.contacts(w).iter().any(|c| self.reached[*c])
                && !self.contacts(w).iter().any(|c| {
                    reached[*c]
                        && !blocked.contains(c)
                        && self.aim(w.unit, *c).is_none_or(|point| {
                            !super::sight::blocked(
                                self.bounds.cell(*c),
                                *point,
                                walls,
                                self.bounds,
                                &self.sight_open,
                            )
                        })
                })
        });
        let answer = sealed.map(|w| w.unit);
        self.checks.insert(key, answer);
        answer
    }
}
