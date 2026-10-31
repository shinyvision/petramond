use crate::fx::HashMap;
use crate::geometry::{cell_of, centre_of, feet_of, offset, reaches, EYE, FACES};
use crate::host::prelude::*;
use crate::worker::tuning::access::{FACE_END_EPS, RAY_EPS, RAY_PAD, SIGHT_POINTS};
use std::sync::Arc;

use super::graph::Cell;
use super::grid::Bounds;

#[derive(Clone, Copy, Debug)]
pub(super) struct Aim {
    pub point: [f64; 3],
    pub potential: bool,
}

pub(super) type Aims = Arc<Vec<(usize, Aim)>>;

#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<([i32; 3], bool), Entry>,
    tick: Option<u64>,
    left: usize,
}

struct Entry {
    targets: Vec<[i32; 3]>,
    record: Option<CellRecord>,
    query: Vec<[i32; 3]>,
    cursor: usize,
    contacts: Vec<usize>,
    aims: Aims,
}

impl Cache {
    pub fn aims(&self, pos: [i32; 3], placing: bool) -> Aims {
        self.entries
            .get(&(pos, placing))
            .map_or_else(|| Arc::new(Vec::new()), |e| Arc::clone(&e.aims))
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

    pub fn changed(&mut self, cells: &[[i32; 3]], lost: bool) {
        if lost {
            self.entries.clear();
            return;
        }
        self.entries.retain(|_, entry| {
            !entry.targets.iter().any(|target| {
                cells.iter().any(|c| {
                    (c[0] - target[0]).abs() <= 5
                        && (c[2] - target[2]).abs() <= 5
                        && (target[1] - 6..=target[1] + 4).contains(&c[1])
                })
            })
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
    ) -> Option<Vec<usize>> {
        if self.tick != Some(now) {
            self.tick = Some(now);
            self.left = SIGHT_POINTS;
        }
        let key = (pos, record.is_some());
        if self
            .entries
            .get(&key)
            .is_some_and(|e| e.targets != targets || e.record != record)
        {
            self.entries.remove(&key);
        }
        let entry = self.entries.entry(key).or_insert_with(|| {
            let mut positions = Vec::new();
            for target in targets {
                for dy in -5..=3 {
                    for dx in -4..=4 {
                        for dz in -4..=4 {
                            positions.push(offset(*target, [dx, dy, dz]));
                        }
                    }
                }
            }
            positions.sort_unstable();
            positions.dedup();
            let mut contacts = Vec::new();
            positions.retain(|s| {
                if targets.contains(s)
                    || targets.contains(&offset(*s, [0, 1, 0]))
                    || (record.is_none() && targets.contains(&offset(*s, [0, -1, 0])))
                    || !reaches(feet_of(*s), targets)
                {
                    return false;
                }
                let Some(i) = bounds.index(*s) else {
                    return false;
                };
                if cells[i].open {
                    return true;
                }
                if (cells[i].dig.is_some() || cells[i].head.is_some())
                    && targets
                        .iter()
                        .any(|t| (s[0] - t[0]).abs() + (s[2] - t[2]).abs() <= 1)
                {
                    contacts.push(i);
                }
                false
            });
            Entry {
                targets: targets.to_vec(),
                record: record.clone(),
                query: positions,
                cursor: 0,
                contacts,
                aims: Arc::new(Vec::new()),
            }
        });
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
                    },
                ));
            }
        }
        self.left -= end - entry.cursor;
        if end == entry.query.len() {
            if !entry.query.is_empty() {
                let aims = Arc::make_mut(&mut entry.aims);
                aims.sort_unstable_by_key(|(node, _)| *node);
                aims.shrink_to_fit();
                entry.contacts.sort_unstable();
                entry.contacts.dedup();
                entry.contacts.shrink_to_fit();
            }
            entry.query = Vec::new();
            entry.cursor = 0;
            Some(entry.contacts.clone())
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
