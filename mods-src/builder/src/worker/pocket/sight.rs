use crate::fx::HashMap;
use crate::geometry::{cell_of, centre_of, feet_of, manhattan, offset, reaches, EYE, FACES};
use crate::host::prelude::*;
use crate::worker::tuning::access::{FACE_END_EPS, RAY_EPS, RAY_PAD, SIGHT_POINTS};
use std::sync::Arc;

use super::graph::Cell;
use super::grid::Bounds;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Aim {
    pub point: [f64; 3],
    pub potential: bool,
    /// The stance found nothing to place the block against.
    pub faceless: bool,
    /// The stance sees a face, but placing from it would turn the block the wrong way.
    pub misaligned: bool,
}

pub(super) type Aims = Arc<Vec<(usize, Aim)>>;

/// The low and high corners of a box of cells.
type Corners = ([i32; 3], [i32; 3]);

#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<([i32; 3], bool), Entry>,
    /// Changed cells not yet applied; one change is often reported several times before the cache is read.
    pending: Vec<[i32; 3]>,
    bounds: Option<Bounds>,
    tick: Option<u64>,
    left: usize,
}

struct Entry {
    targets: Vec<[i32; 3]>,
    /// Corners of the box the targets fill.
    low: [i32; 3],
    high: [i32; 3],
    record: Option<CellRecord>,
    /// Every stance the targets' reach admits, whatever stands there now.
    stances: Vec<[i32; 3]>,
    /// Corners of the box the stances fill.
    stances_low: [i32; 3],
    stances_high: [i32; 3],
    /// Stances to classify against the current cells before asking.
    redo: Vec<[i32; 3]>,
    query: Vec<[i32; 3]>,
    cursor: usize,
    contacts: Vec<usize>,
    /// `contacts` as handed out, until the entry changes again.
    shared: Option<Arc<[usize]>>,
    aims: Aims,
}

impl Entry {
    fn new(bounds: Bounds, targets: &[[i32; 3]], record: Option<CellRecord>) -> Self {
        let mut low = [i32::MAX; 3];
        let mut high = [i32::MIN; 3];
        for t in targets {
            for a in 0..3 {
                low[a] = low[a].min(t[a]);
                high[a] = high[a].max(t[a]);
            }
        }
        // Offsets in x, y, z order keep one target's stances sorted.
        let mut stances = Vec::new();
        for target in targets {
            for dx in -4..=4 {
                for dy in -5..=3 {
                    for dz in -4..=4 {
                        stances.push(offset(*target, [dx, dy, dz]));
                    }
                }
            }
        }
        if targets.len() > 1 {
            stances.sort_unstable();
            stances.dedup();
        }
        stances.retain(|s| {
            !(targets.contains(s)
                || targets.contains(&offset(*s, [0, 1, 0]))
                || (record.is_none() && targets.contains(&offset(*s, [0, -1, 0])))
                || !reaches(feet_of(*s), targets)
                || bounds.index(*s).is_none())
        });
        let mut stances_low = [i32::MAX; 3];
        let mut stances_high = [i32::MIN; 3];
        for s in &stances {
            for a in 0..3 {
                stances_low[a] = stances_low[a].min(s[a]);
                stances_high[a] = stances_high[a].max(s[a]);
            }
        }
        Self {
            targets: targets.to_vec(),
            low,
            high,
            record,
            stances_low,
            stances_high,
            redo: stances.clone(),
            stances,
            query: Vec::new(),
            cursor: 0,
            contacts: Vec::new(),
            shared: None,
            aims: Arc::new(Vec::new()),
        }
    }

    /// Whether `cell` touches the targets' own faces, which decides every stance's answer.
    fn faces(&self, cell: [i32; 3]) -> bool {
        self.targets.iter().any(|t| manhattan(*t, cell) <= 1)
    }

    /// Whether `cell` lies close enough to a target for a change there to matter.
    fn near(&self, c: [i32; 3]) -> bool {
        let (low, high) = (self.low, self.high);
        if c[0] < low[0] - 5
            || c[0] > high[0] + 5
            || c[2] < low[2] - 5
            || c[2] > high[2] + 5
            || c[1] < low[1] - 6
            || c[1] > high[1] + 4
        {
            return false;
        }
        self.targets.iter().any(|target| {
            (c[0] - target[0]).abs() <= 5
                && (c[2] - target[2]).abs() <= 5
                && (target[1] - 6..=target[1] + 4).contains(&c[1])
        })
    }

    /// The stances whose box between eye and targets holds `cell`, as a box of stance
    /// coordinates. The box test splits by axis, each axis depends on one stance coordinate, and
    /// both ends of the eye's span grow with it, so the coordinates that pass form a range.
    fn sightline_box(&self, cell: [i32; 3]) -> Corners {
        let (from, to) = (self.stances_low, self.stances_high);
        let mut low = [i32::MAX; 3];
        let mut high = [i32::MIN; 3];
        for a in 0..3 {
            for v in from[a]..=to[a] {
                let mut stance = [0; 3];
                stance[a] = v;
                let mut eye = feet_of(stance);
                eye[1] += EYE;
                let start = eye[a].min(f64::from(self.low[a])) - RAY_PAD;
                let end = eye[a].max(f64::from(self.high[a]) + 1.0) + RAY_PAD;
                if (start.floor() as i32..=end.floor() as i32).contains(&cell[a]) {
                    low[a] = low[a].min(v);
                    high[a] = high[a].max(v);
                }
            }
        }
        (low, high)
    }

    /// Re-asks the stances whose footing, body or line of sight to the targets can cross one of `cells`.
    fn forget_crossing(&mut self, bounds: Bounds, cells: &[[i32; 3]]) {
        if self.stances.is_empty() {
            return;
        }
        let tests: Vec<([i32; 3], Corners)> =
            cells.iter().map(|c| (*c, self.sightline_box(*c))).collect();
        let crosses = |s: [i32; 3]| {
            tests.iter().any(|(cell, (low, high))| {
                (s[0] == cell[0] && s[2] == cell[2] && (s[1] - 1..=s[1] + 1).contains(&cell[1]))
                    || (low[0] <= s[0]
                        && s[0] <= high[0]
                        && low[1] <= s[1]
                        && s[1] <= high[1]
                        && low[2] <= s[2]
                        && s[2] <= high[2])
            })
        };
        let affected: Vec<[i32; 3]> = self
            .stances
            .iter()
            .copied()
            .filter(|s| crosses(*s))
            .collect();
        if affected.is_empty() {
            return;
        }
        // Contacts, aims and queries all name stances, so the same test picks them out.
        self.contacts.retain(|n| !crosses(bounds.cell(*n)));
        self.shared = None;
        if self.aims.iter().any(|(n, _)| crosses(bounds.cell(*n))) {
            Arc::make_mut(&mut self.aims).retain(|(n, _)| !crosses(bounds.cell(*n)));
        }
        let pending = self.query.split_off(self.cursor.min(self.query.len()));
        self.query
            .extend(pending.into_iter().filter(|s| !crosses(*s)));
        self.redo.extend(affected);
    }
}

impl Cache {
    pub fn aims(&mut self, pos: [i32; 3], placing: bool) -> Aims {
        self.apply_changes();
        self.entries
            .get(&(pos, placing))
            .map_or_else(|| Arc::new(Vec::new()), |e| Arc::clone(&e.aims))
    }

    /// A finished answer that no change has reopened; a key's targets never change, so it needs no comparison.
    pub fn settled(&mut self, pos: [i32; 3], placing: bool) -> Option<(Arc<[usize]>, Aims)> {
        self.apply_changes();
        let entry = self.entries.get(&(pos, placing))?;
        let shared = entry.shared.as_ref()?;
        (entry.redo.is_empty() && entry.query.is_empty())
            .then(|| (Arc::clone(shared), Arc::clone(&entry.aims)))
    }

    pub fn retain(&mut self, active: &crate::fx::HashSet<([i32; 3], bool)>) {
        self.apply_changes();
        self.entries.retain(|key, _| active.contains(key));
    }
    #[cfg(test)]
    pub fn unbudget(&mut self, now: u64) {
        self.budget(now, usize::MAX);
    }
    #[cfg(test)]
    pub fn budget(&mut self, now: u64, left: usize) {
        self.tick = Some(now);
        self.left = left;
    }

    /// Re-asks only the stances a changed cell can affect; a change at a target's faces re-asks the target whole.
    pub fn changed(&mut self, cells: &[[i32; 3]], lost: bool) {
        if lost {
            self.entries.clear();
            self.pending.clear();
            return;
        }
        let Some(bounds) = self.bounds else {
            return;
        };
        // Targets lie inside the bounds, so nothing further out is near one.
        self.pending.extend(cells.iter().copied().filter(|c| {
            (0..3).all(|a| {
                c[a] >= bounds.min[a] - 6 && c[a] < bounds.min[a] + bounds.size[a] as i32 + 6
            })
        }));
    }

    fn apply_changes(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let Some(bounds) = self.bounds else {
            self.pending.clear();
            return;
        };
        let mut cells = std::mem::take(&mut self.pending);
        cells.sort_unstable();
        cells.dedup();
        let mut near = Vec::new();
        self.entries.retain(|_, entry| {
            near.clear();
            near.extend(cells.iter().copied().filter(|c| entry.near(*c)));
            if near.is_empty() {
                return true;
            }
            if near.iter().any(|c| entry.faces(*c)) {
                return false;
            }
            entry.forget_crossing(bounds, &near);
            true
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn contacts(
        &mut self,
        now: u64,
        bounds: Bounds,
        cells: &[Cell],
        raw: &[Cell],
        actor: EntityRef,
        targets: &[[i32; 3]],
        pos: [i32; 3],
        record: Option<CellRecord>,
    ) -> Option<Arc<[usize]>> {
        if self.tick != Some(now) {
            self.tick = Some(now);
            self.left = SIGHT_POINTS;
        }
        self.apply_changes();
        if self.bounds != Some(bounds) {
            self.entries.clear();
            self.bounds = Some(bounds);
        }
        let key = (pos, record.is_some());
        if self
            .entries
            .get(&key)
            .is_some_and(|e| e.targets != targets || e.record != record)
        {
            self.entries.remove(&key);
        }
        let entry = self
            .entries
            .entry(key)
            .or_insert_with(|| Entry::new(bounds, targets, record.clone()));
        if !entry.redo.is_empty() {
            entry.shared = None;
            let mut redo = std::mem::take(&mut entry.redo);
            redo.sort_unstable();
            redo.dedup();
            for s in redo {
                let i = bounds.index(s).unwrap();
                if cells[i].open {
                    entry.query.push(s);
                } else if (cells[i].dig.is_some() || cells[i].head.is_some())
                    && targets
                        .iter()
                        .any(|t| (s[0] - t[0]).abs() + (s[2] - t[2]).abs() <= 1)
                {
                    entry.contacts.push(i);
                }
            }
        }
        let end = entry.cursor + self.left.min(entry.query.len() - entry.cursor);
        let positions = &entry.query[entry.cursor..end];
        let answers = paged(positions.iter().map(|p| feet_of(*p)).collect(), |from| {
            actor_aims(actor, from, pos, record.clone())
        });
        for (p, answer) in positions.iter().zip(answers) {
            let visible = match answer {
                Ok(_) => true,
                Err(
                    ActionRefusal::NoFace
                    | ActionRefusal::NoSupport
                    | ActionRefusal::Misaligned
                    | ActionRefusal::MissingItems
                    | ActionRefusal::BodyInTheWay,
                ) => {
                    let mut eye = feet_of(*p);
                    eye[1] += EYE;
                    record.is_some() && {
                        let mut clear = true;
                        super::rays::walk(eye, centre_of(pos), |c| {
                            clear = targets.contains(&c)
                                || bounds.index(c).is_some_and(|i| raw[i].open || raw[i].sight);
                            clear
                        });
                        clear
                    }
                }
                Err(_) => false,
            };
            if visible {
                let node = bounds.index(*p).unwrap();
                entry.contacts.push(node);
                Arc::make_mut(&mut entry.aims).push((
                    node,
                    Aim {
                        point: answer.unwrap_or_else(|_| centre_of(pos)),
                        potential: answer.is_err(),
                        faceless: matches!(
                            answer,
                            Err(ActionRefusal::NoFace | ActionRefusal::NoSupport)
                        ),
                        misaligned: answer == Err(ActionRefusal::Misaligned),
                    },
                ));
            }
        }
        self.left -= end - entry.cursor;
        if end == entry.query.len() {
            if !entry.query.is_empty() || !entry.contacts.is_sorted() {
                // Stable sorts merge the sorted answers kept with the new ones.
                let aims = Arc::make_mut(&mut entry.aims);
                aims.sort_by_key(|(node, _)| *node);
                aims.shrink_to_fit();
                entry.contacts.sort();
                entry.contacts.dedup();
                entry.contacts.shrink_to_fit();
            }
            entry.query = Vec::new();
            entry.cursor = 0;
            Some(Arc::clone(
                entry
                    .shared
                    .get_or_insert_with(|| Arc::from(entry.contacts.as_slice())),
            ))
        } else {
            entry.cursor = end;
            None
        }
    }
}

pub(super) fn blocked(
    stance: [i32; 3],
    aim: Aim,
    walls: &[[i32; 3]],
    bounds: Bounds,
    sight_open: &[bool],
) -> bool {
    if aim.potential {
        let target = cell_of(aim.point);
        let faces: Vec<_> = FACES
            .iter()
            .filter_map(|side| {
                let wall = offset(target, *side);
                walls.contains(&wall).then(|| {
                    let centre = centre_of(wall);
                    std::array::from_fn(|a| centre[a] - f64::from(side[a]) * 0.5)
                })
            })
            .collect();
        if !faces.is_empty() {
            let mut eye = feet_of(stance);
            eye[1] += EYE;
            return faces.iter().all(|face| {
                segment_blocked(stance, *face, walls, false) || {
                    let mut hit = false;
                    super::rays::walk(eye, *face, |cell| {
                        hit = bounds.index(cell).is_none_or(|i| !sight_open[i]);
                        !hit
                    });
                    hit
                }
            });
        }
    }
    segment_blocked(stance, aim.point, walls, true)
}

fn segment_blocked(stance: [i32; 3], point: [f64; 3], walls: &[[i32; 3]], endpoint: bool) -> bool {
    let mut from = feet_of(stance);
    from[1] += EYE;
    walls.iter().any(|wall| {
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for a in 0..3 {
            let min = f64::from(wall[a]) - RAY_PAD;
            let max = f64::from(wall[a]) + 1.0 + RAY_PAD;
            let d = point[a] - from[a];
            if d.abs() < RAY_EPS {
                if from[a] < min || from[a] > max {
                    return false;
                }
            } else {
                let first = (min - from[a]) / d;
                let last = (max - from[a]) / d;
                lo = lo.max(first.min(last));
                hi = hi.min(first.max(last));
            }
        }
        lo <= hi + RAY_EPS
            && hi > RAY_EPS
            && if endpoint {
                lo <= 1.0
            } else {
                lo < 1.0 - FACE_END_EPS
            }
    })
}
