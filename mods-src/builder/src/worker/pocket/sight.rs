use crate::fx::{HashMap, HashSet};
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

#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<([i32; 3], bool), Entry>,
    bounds: Option<Bounds>,
    tick: Option<u64>,
    left: usize,
}

struct Entry {
    targets: Vec<[i32; 3]>,
    record: Option<CellRecord>,
    /// Every stance the targets' reach admits, whatever stands there now.
    stances: Vec<[i32; 3]>,
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
        let mut stances = Vec::new();
        for target in targets {
            for dy in -5..=3 {
                for dx in -4..=4 {
                    for dz in -4..=4 {
                        stances.push(offset(*target, [dx, dy, dz]));
                    }
                }
            }
        }
        stances.sort_unstable();
        stances.dedup();
        stances.retain(|s| {
            !(targets.contains(s)
                || targets.contains(&offset(*s, [0, 1, 0]))
                || (record.is_none() && targets.contains(&offset(*s, [0, -1, 0])))
                || !reaches(feet_of(*s), targets)
                || bounds.index(*s).is_none())
        });
        Self {
            targets: targets.to_vec(),
            record,
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

    /// Stances whose footing, body or line of sight to the targets can cross `cell`.
    fn crossing(&self, cell: [i32; 3]) -> impl Iterator<Item = [i32; 3]> + '_ {
        let mut low = [i32::MAX; 3];
        let mut high = [i32::MIN; 3];
        for t in &self.targets {
            for a in 0..3 {
                low[a] = low[a].min(t[a]);
                high[a] = high[a].max(t[a]);
            }
        }
        self.stances.iter().copied().filter(move |s| {
            if s[0] == cell[0] && s[2] == cell[2] && (s[1] - 1..=s[1] + 1).contains(&cell[1]) {
                return true;
            }
            let mut eye = feet_of(*s);
            eye[1] += EYE;
            (0..3).all(|a| {
                let from = eye[a].min(f64::from(low[a])) - RAY_PAD;
                let to = eye[a].max(f64::from(high[a]) + 1.0) + RAY_PAD;
                (from.floor() as i32..=to.floor() as i32).contains(&cell[a])
            })
        })
    }

    fn forget(&mut self, bounds: Bounds, stances: &HashSet<[i32; 3]>) {
        let nodes: HashSet<usize> = stances.iter().filter_map(|s| bounds.index(*s)).collect();
        self.contacts.retain(|n| !nodes.contains(n));
        self.shared = None;
        if self.aims.iter().any(|(n, _)| nodes.contains(n)) {
            Arc::make_mut(&mut self.aims).retain(|(n, _)| !nodes.contains(n));
        }
        let pending = self.query.split_off(self.cursor.min(self.query.len()));
        self.query
            .extend(pending.into_iter().filter(|s| !stances.contains(s)));
        self.redo.extend(stances.iter().copied());
    }
}

impl Cache {
    pub fn aims(&self, pos: [i32; 3], placing: bool) -> Aims {
        self.entries
            .get(&(pos, placing))
            .map_or_else(|| Arc::new(Vec::new()), |e| Arc::clone(&e.aims))
    }

    /// A finished answer that no change has reopened; a key's targets never change, so it needs no comparison.
    pub fn settled(&self, pos: [i32; 3], placing: bool) -> Option<(Arc<[usize]>, Aims)> {
        let entry = self.entries.get(&(pos, placing))?;
        let shared = entry.shared.as_ref()?;
        (entry.redo.is_empty() && entry.query.is_empty())
            .then(|| (Arc::clone(shared), Arc::clone(&entry.aims)))
    }

    pub fn retain(&mut self, active: &crate::fx::HashSet<([i32; 3], bool)>) {
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
            return;
        }
        let Some(bounds) = self.bounds else {
            return;
        };
        self.entries.retain(|_, entry| {
            let near: Vec<[i32; 3]> = cells
                .iter()
                .copied()
                .filter(|c| {
                    entry.targets.iter().any(|target| {
                        (c[0] - target[0]).abs() <= 5
                            && (c[2] - target[2]).abs() <= 5
                            && (target[1] - 6..=target[1] + 4).contains(&c[1])
                    })
                })
                .collect();
            if near.iter().any(|c| entry.faces(*c)) {
                return false;
            }
            let affected: HashSet<[i32; 3]> =
                near.iter().flat_map(|c| entry.crossing(*c)).collect();
            if !affected.is_empty() {
                entry.forget(bounds, &affected);
            }
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
                    record.is_some()
                        && super::rays::crossed(eye, centre_of(pos)).iter().all(|c| {
                            targets.contains(c)
                                || bounds
                                    .index(*c)
                                    .is_some_and(|i| raw[i].open || raw[i].sight)
                        })
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
                let aims = Arc::make_mut(&mut entry.aims);
                aims.sort_unstable_by_key(|(node, _)| *node);
                aims.shrink_to_fit();
                entry.contacts.sort_unstable();
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
                segment_blocked(stance, *face, walls, false)
                    || super::rays::crossed(eye, *face)
                        .iter()
                        .any(|cell| bounds.index(*cell).is_none_or(|i| !sight_open[i]))
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
