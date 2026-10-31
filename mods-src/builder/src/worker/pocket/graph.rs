use std::collections::VecDeque;

use crate::fx::{HashMap, HashSet};
use crate::geometry::offset;
use crate::worker::Task;

use super::grid::Bounds;

type Check = (Option<usize>, Vec<usize>, Vec<[i32; 3]>);

/// A ground check's unit, body, walls, the worker's perch, and the stamps of the floods it reads.
pub(super) type GroundCheck = (
    Option<usize>,
    Vec<[i32; 3]>,
    Vec<[i32; 3]>,
    Option<[i32; 3]>,
    [u64; 4],
);

/// Side of the cubes work is filed under.
const AREA: i32 = 8;
/// How far work's other contacts and sightlines can lie from its first contact: every contact
/// sits in the stance box around a unit up to two cells long, and every sightline ends at it.
const SPREAD: i32 = 12;

/// Work filed by the area of its first contact, so a check visits only the work near what changed.
struct Nearby {
    by_area: HashMap<[i32; 3], Vec<usize>>,
}

/// World changes after the graph was built: cells closed since then stand as walls, finished units need no access.
#[derive(Default)]
struct Since {
    body: Vec<[i32; 3]>,
    walls: Vec<[i32; 3]>,
    done: HashSet<usize>,
    /// Reached cells still connected to home once `body` is closed; `None` while nothing is.
    after: Option<Vec<bool>>,
    /// Reached cells `body` closed or cut off.
    moved: Vec<usize>,
}

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Cell {
    pub open: bool,
    pub sight: bool,
    pub dig: Option<(Task, u32)>,
    pub head: Option<(Task, u32)>,
}

pub(super) struct Work {
    pub unit: usize,
    pub contacts: std::sync::Arc<[usize]>,
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
    /// Stances with a round trip from home, by cell.
    pub ground: Vec<bool>,
    pub supports: super::supports::Supports,
    /// Raw cells open when the graph was built; later closures are judged against this.
    pub raw_open: Vec<bool>,
    since: Since,
    nearby: Option<Nearby>,
    /// Work no stance can place yet for want of a face; it cannot veto the placement that may give it one.
    faceless: HashSet<usize>,
    /// Each unit's aims as last judged for `faceless`, kept across graphs: aims are shared and
    /// copied on change, so the same allocation means the same answer.
    pub faceless_judged: HashMap<usize, (super::sight::Aims, bool)>,
    #[cfg(test)]
    pub scan_all: bool,
    physical: HashMap<usize, Vec<usize>>,
    checks: HashMap<Check, Option<usize>>,
    ground_checks: HashMap<GroundCheck, Option<usize>>,
}

impl Graph {
    pub fn new(bounds: Bounds, cells: Vec<Cell>, work: Vec<Work>, root: usize) -> Self {
        let len = cells.len();
        let mut graph = Self {
            bounds,
            reached: vec![false; cells.len()],
            components: vec![usize::MAX; cells.len()],
            component_count: 0,
            sight_open: cells.iter().map(|c| c.open || c.sight).collect(),
            cells,
            work,
            root,
            sealed: Vec::new(),
            checks: HashMap::default(),
            ground_checks: HashMap::default(),
            aims: HashMap::default(),
            ground: vec![false; len],
            supports: Default::default(),
            raw_open: Vec::new(),
            since: Since::default(),
            nearby: None,
            faceless: HashSet::default(),
            faceless_judged: HashMap::default(),
            #[cfg(test)]
            scan_all: false,
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
        graph
    }

    #[cfg(test)]
    pub fn set_aim(&mut self, unit: usize, node: usize, aim: super::sight::Aim) {
        let points = self.aims.entry(unit).or_default();
        let points = std::sync::Arc::make_mut(points);
        points.push((node, aim));
        points.sort_unstable_by_key(|(node, _)| *node);
        self.nearby = None;
        self.note_faceless();
    }

    fn note_faceless(&mut self) {
        let mut judged = std::mem::take(&mut self.faceless_judged);
        let mut faceless = HashSet::default();
        for (unit, points) in &self.aims {
            let known = judged
                .remove(unit)
                .filter(|(seen, _)| std::sync::Arc::ptr_eq(seen, points))
                .map(|(_, answer)| answer);
            let answer = known.unwrap_or_else(|| {
                !points.is_empty() && points.iter().all(|(_, aim)| aim.faceless)
            });
            if answer {
                faceless.insert(*unit);
            }
            self.faceless_judged
                .insert(*unit, (std::sync::Arc::clone(points), answer));
        }
        self.faceless = faceless;
    }

    fn aim(&self, unit: usize, node: usize) -> Option<&super::sight::Aim> {
        aim_in(self.aims.get(&unit), node)
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
        self.ground_checks.clear();
        self.nearby = None;
        self.note_faceless();
    }

    fn file_nearby(&mut self) {
        if self.nearby.is_some() {
            return;
        }
        let mut by_area: HashMap<[i32; 3], Vec<usize>> = HashMap::default();
        for (i, w) in self.work.iter().enumerate() {
            if let Some(first) = self.contacts(w).first() {
                let area = self.bounds.cell(*first).map(|v| v.div_euclid(AREA));
                by_area.entry(area).or_default().push(i);
            }
        }
        self.nearby = Some(Nearby { by_area });
    }

    /// Work near any of `cells` or `walls`, in work order.
    fn touched(&self, cells: &[usize], walls: &[[i32; 3]]) -> Vec<usize> {
        let nearby = self.nearby.as_ref().unwrap();
        let mut areas: Vec<[i32; 3]> = Vec::new();
        for at in cells
            .iter()
            .map(|c| self.bounds.cell(*c))
            .chain(walls.iter().copied())
        {
            let low = at.map(|v| (v - SPREAD).div_euclid(AREA));
            let high = at.map(|v| (v + SPREAD).div_euclid(AREA));
            for x in low[0]..=high[0] {
                for y in low[1]..=high[1] {
                    for z in low[2]..=high[2] {
                        areas.push([x, y, z]);
                    }
                }
            }
        }
        areas.sort_unstable();
        areas.dedup();
        let mut touched: Vec<usize> = areas
            .iter()
            .filter_map(|a| nearby.by_area.get(a))
            .flatten()
            .copied()
            .collect();
        touched.sort_unstable();
        touched.dedup();
        touched
    }

    pub fn verified(&self, unit: usize, cell: [i32; 3]) -> bool {
        self.physical.get(&unit).is_none_or(|contacts| {
            self.bounds
                .index(cell)
                .is_some_and(|i| contacts.contains(&i))
        })
    }

    /// Lays changes made after the graph was built over it: `closed` cells now bar bodies, `walls` now bar sight.
    pub fn since(&mut self, closed: &[[i32; 3]], mut walls: Vec<[i32; 3]>, done: HashSet<usize>) {
        let mut body = closed.to_vec();
        body.extend(closed.iter().map(|c| offset(*c, [0, -1, 0])));
        body.sort_unstable();
        body.dedup();
        walls.sort_unstable();
        if body == self.since.body && walls == self.since.walls && done == self.since.done {
            return;
        }
        let mut closed: Vec<usize> = body
            .iter()
            .filter_map(|c| self.bounds.index(*c))
            .filter(|i| self.reached[*i])
            .collect();
        closed.sort_unstable();
        let (after, moved) = if closed.is_empty() {
            (None, Vec::new())
        } else {
            let after = match self.detached(&self.reached, &closed) {
                Some(lost) => {
                    let mut after = self.reached.clone();
                    for c in closed.iter().chain(&lost) {
                        after[*c] = false;
                    }
                    after
                }
                None => self.flood_without(&self.reached, &closed),
            };
            let moved = (0..after.len())
                .filter(|c| self.reached[*c] && !after[*c])
                .collect();
            (Some(after), moved)
        };
        self.since = Since {
            body,
            walls,
            done,
            after,
            moved,
        };
        self.checks.clear();
        self.ground_checks.clear();
    }

    fn merged<'a>(extra: &[[i32; 3]], cells: &'a [[i32; 3]]) -> std::borrow::Cow<'a, [[i32; 3]]> {
        if extra.is_empty() {
            return std::borrow::Cow::Borrowed(cells);
        }
        let mut all = cells.to_vec();
        all.extend_from_slice(extra);
        all.sort_unstable();
        all.dedup();
        std::borrow::Cow::Owned(all)
    }

    pub fn needs_ground(&self) -> bool {
        !self.physical.is_empty()
    }

    /// Work the closure of `body` and `walls` leaves without any contact the worker gets to,
    /// counting only work that has one before it: what the worker already cannot reach is
    /// nothing this closure takes away.
    pub fn seals_ground(
        &self,
        except: Option<usize>,
        body: &[[i32; 3]],
        walls: &[[i32; 3]],
        reachable: impl Fn([i32; 3]) -> bool,
        reached_before: impl Fn([i32; 3]) -> bool,
    ) -> Option<usize> {
        let after_body = Self::merged(&self.since.body, body);
        let after_walls = Self::merged(&self.since.walls, walls);
        let usable = |w: &Work,
                      c: usize,
                      body: &[[i32; 3]],
                      walls: &[[i32; 3]],
                      reachable: &dyn Fn([i32; 3]) -> bool| {
            let cell = self.bounds.cell(c);
            let supported = self.supports.get(c).is_some_and(|base| {
                let base = self.bounds.cell(base);
                reachable(base)
                    && !walls.iter().any(|wall| {
                        wall[0] == cell[0]
                            && wall[2] == cell[2]
                            && (base[1]..=cell[1] + 1).contains(&wall[1])
                    })
            });
            !body.contains(&cell)
                && (reachable(cell) || supported)
                && self.aim(w.unit, c).is_none_or(|aim| {
                    !super::sight::blocked(cell, *aim, walls, self.bounds, &self.sight_open)
                })
        };
        self.work
            .iter()
            .find(|w| {
                Some(w.unit) != except
                    && !self.since.done.contains(&w.unit)
                    && !self.faceless.contains(&w.unit)
                    && self.physical.contains_key(&w.unit)
                    && self.contacts(w).iter().any(|c| self.reached[*c])
                    && self.contacts(w).iter().any(|c| {
                        usable(w, *c, &self.since.body, &self.since.walls, &reached_before)
                    })
                    && !self
                        .contacts(w)
                        .iter()
                        .any(|c| usable(w, *c, &after_body, &after_walls, &reachable))
            })
            .map(|w| w.unit)
    }

    /// `seals_ground`, remembered while the graph and the floods the key names stand.
    pub fn seals_ground_remembered(
        &mut self,
        key: GroundCheck,
        reachable: impl Fn([i32; 3]) -> bool,
        reached_before: impl Fn([i32; 3]) -> bool,
    ) -> Option<usize> {
        if let Some(answer) = self.ground_checks.get(&key) {
            return *answer;
        }
        let answer = self.seals_ground(key.0, &key.1, &key.2, reachable, reached_before);
        self.ground_checks.insert(key, answer);
        answer
    }

    fn contacts<'a>(&'a self, work: &'a Work) -> &'a [usize] {
        self.physical
            .get(&work.unit)
            .map_or(&work.contacts[..], |c| c.as_slice())
    }

    fn components(&mut self) {
        const UNSEEN: usize = usize::MAX - 1;
        let [sx, _, sz] = self.bounds.size;
        let layer = sx * sz;
        let components = &mut self.components;
        for (i, cell) in self.cells.iter().enumerate() {
            if cell.open {
                components[i] = UNSEEN;
            }
        }
        let len = components.len();
        let mut queue = Vec::new();
        for start in 0..len {
            if components[start] != UNSEEN {
                continue;
            }
            let id = self.component_count;
            queue.clear();
            queue.push(start);
            components[start] = id;
            let mut at = 0;
            while at < queue.len() {
                let cell = queue[at];
                at += 1;
                let x = cell % sx;
                let z = cell / sx % sz;
                let mut visit = |next: usize| {
                    if components[next] == UNSEEN {
                        components[next] = id;
                        queue.push(next);
                    }
                };
                if x + 1 < sx {
                    visit(cell + 1);
                }
                if x > 0 {
                    visit(cell - 1);
                }
                if cell + layer < len {
                    visit(cell + layer);
                }
                if cell >= layer {
                    visit(cell - layer);
                }
                if z + 1 < sz {
                    visit(cell + sx);
                }
                if z > 0 {
                    visit(cell - sx);
                }
            }
            self.component_count += 1;
        }
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
        let own_walls = walls;
        let walls = Self::merged(&self.since.walls, walls);
        let walls = &*walls;
        let base = self.since.after.as_deref().unwrap_or(&self.reached);
        let mut blocked: Vec<usize> = cells
            .iter()
            .filter_map(|c| self.bounds.index(*c))
            .filter(|i| base[*i])
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
        self.file_nearby();
        let base = self.since.after.as_deref().unwrap_or(&self.reached);
        let local = if blocked.is_empty() {
            Some(HashSet::default())
        } else {
            self.detached(base, blocked)
        };
        let whole = local.is_none().then(|| self.flood_without(base, blocked));
        let keeps = |c: usize| match (&local, &whole) {
            (Some(lost), _) => base[c] && !lost.contains(&c),
            (None, Some(after)) => after[c],
            (None, None) => unreachable!(),
        };
        let works = |points: Option<&super::sight::Aims>, c: usize| {
            keeps(c)
                && blocked.binary_search(&c).is_err()
                && aim_in(points, c).is_none_or(|point| {
                    !super::sight::blocked(
                        self.bounds.cell(c),
                        *point,
                        walls,
                        self.bounds,
                        &self.sight_open,
                    )
                })
        };
        // A stance that would set the block the wrong way is no way to build it: losing the last
        // stance that sets it right seals it as surely as losing every stance.
        let aligned = |points: Option<&super::sight::Aims>, c: usize| {
            aim_in(points, c).is_none_or(|aim| !aim.misaligned && !aim.faceless)
        };
        let sealed_by = |w: &&Work| {
            if Some(w.unit) == except
                || self.since.done.contains(&w.unit)
                || self.faceless.contains(&w.unit)
            {
                return false;
            }
            let contacts = self.contacts(w);
            if !contacts.iter().any(|c| self.reached[*c]) {
                return false;
            }
            let points = self.aims.get(&w.unit);
            !contacts.iter().any(|c| works(points, *c))
                || (contacts
                    .iter()
                    .any(|c| self.reached[*c] && aligned(points, *c))
                    && !contacts
                        .iter()
                        .any(|c| works(points, *c) && aligned(points, *c)))
        };
        // Work away from every change keeps the contact it had: unchanged, every reached contact
        // works, and an out-of-date graph is consulted only once its closures alone seal nothing.
        let full = whole.is_some();
        #[cfg(test)]
        let full = full || self.scan_all;
        let answer = if full {
            self.work.iter().find(sealed_by)
        } else {
            let lost = local.as_ref().unwrap();
            let (cells, walls): (Vec<usize>, &[[i32; 3]]) =
                if blocked.is_empty() && own_walls.is_empty() {
                    (self.since.moved.clone(), &self.since.walls)
                } else {
                    (blocked.iter().chain(lost).copied().collect(), own_walls)
                };
            self.touched(&cells, walls)
                .into_iter()
                .map(|i| &self.work[i])
                .find(sealed_by)
        }
        .map(|w| w.unit);
        self.checks.insert(key, answer);
        answer
    }

    /// Reached cells that `blocked` cuts off from home, found by flooding from every open
    /// neighbour of `blocked` at once until all floods meet: a flood that runs dry first holds a
    /// detached region. `None` when home itself ends up cut off, so its side is the small one.
    pub(super) fn detached(&self, base: &[bool], blocked: &[usize]) -> Option<HashSet<usize>> {
        if !base[self.root] || blocked.binary_search(&self.root).is_ok() {
            return None;
        }
        let open = |n: &usize| base[*n] && blocked.binary_search(n).is_err();
        let mut seeds: Vec<usize> = blocked
            .iter()
            .flat_map(|b| self.bounds.neighbours(*b))
            .flatten()
            .filter(open)
            .collect();
        seeds.sort_unstable();
        seeds.dedup();
        let mut owner: HashMap<usize, usize> =
            seeds.iter().enumerate().map(|(g, s)| (*s, g)).collect();
        let mut group: Vec<usize> = (0..seeds.len()).collect();
        let mut queues: Vec<VecDeque<usize>> = seeds.iter().map(|s| VecDeque::from([*s])).collect();
        let find = |group: &mut Vec<usize>, mut g: usize| {
            while group[g] != g {
                group[g] = group[group[g]];
                g = group[g];
            }
            g
        };
        let mut running = seeds.len();
        let mut lost = HashSet::default();
        while running > 1 {
            for g in 0..seeds.len() {
                if group[g] != g || queues[g].is_empty() {
                    continue;
                }
                let cell = queues[g].pop_front().unwrap();
                for n in self.bounds.neighbours(cell).into_iter().flatten() {
                    if !open(&n) {
                        continue;
                    }
                    match owner.get(&n).copied() {
                        None => {
                            owner.insert(n, g);
                            queues[g].push_back(n);
                        }
                        Some(other) => {
                            let other = find(&mut group, other);
                            if other != g {
                                group[other] = g;
                                let moved = std::mem::take(&mut queues[other]);
                                queues[g].extend(moved);
                                running -= 1;
                            }
                        }
                    }
                }
                if queues[g].is_empty() {
                    let mut side = Vec::new();
                    for (&cell, &o) in &owner {
                        if find(&mut group, o) == g {
                            side.push(cell);
                        }
                    }
                    if side.contains(&self.root) {
                        return None;
                    }
                    lost.extend(side);
                    running -= 1;
                }
                if running <= 1 {
                    break;
                }
            }
        }
        Some(lost)
    }

    /// Cells of `base` still connected to home once `blocked` is closed.
    pub(super) fn flood_without(&self, base: &[bool], blocked: &[usize]) -> Vec<bool> {
        let mut after = vec![false; self.cells.len()];
        let mut queue = Vec::new();
        if base[self.root] && blocked.binary_search(&self.root).is_err() {
            queue.push(self.root);
            after[self.root] = true;
        }
        let mut at = 0;
        while at < queue.len() {
            let cell = queue[at];
            at += 1;
            for next in self.bounds.neighbours(cell).into_iter().flatten() {
                if base[next] && !after[next] && blocked.binary_search(&next).is_err() {
                    after[next] = true;
                    queue.push(next);
                }
            }
        }
        after
    }
}

/// The aim at `node` among one unit's aims.
fn aim_in(points: Option<&super::sight::Aims>, node: usize) -> Option<&super::sight::Aim> {
    let points = points?;
    let i = points.binary_search_by_key(&node, |(node, _)| *node).ok()?;
    Some(&points[i].1)
}
