use std::collections::VecDeque;

use crate::fx::{HashMap, HashSet};
use crate::geometry::{offset, FACES, SIDES};
use crate::host::prelude::*;

use super::State;

pub const REACH: f64 = 4.5;
pub const EYE: f64 = 1.3;
const DROP: i32 = 3;

pub struct Ground<'a> {
    pub state: &'a State,
    pub blocked: HashSet<[i32; 3]>,
}

impl<'a> Ground<'a> {
    pub fn new(state: &'a State, blocked: &[[i32; 3]]) -> Self {
        Self {
            state,
            blocked: blocked.iter().copied().collect(),
        }
    }

    fn clear(&self, cell: [i32; 3]) -> bool {
        self.state.block_at(cell).is_some_and(|block| {
            let info = &self.state.row(block).info;
            !self.blocked.contains(&cell) && info.collision.is_empty() && info.fluid.is_none()
        })
    }

    fn floor(&self, cell: [i32; 3]) -> bool {
        self.state.block_at(cell).is_some_and(|block| {
            self.blocked.contains(&cell) || !self.state.row(block).info.collision.is_empty()
        })
    }

    pub fn foothold(&self, cell: [i32; 3]) -> bool {
        self.clear(cell)
            && self.clear(offset(cell, [0, 1, 0]))
            && self.floor(offset(cell, [0, -1, 0]))
    }

    pub fn moves(&self, cell: [i32; 3]) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        for side in SIDES {
            let n = offset(cell, side);
            if self.foothold(n) {
                out.push(n);
                continue;
            }
            let up = offset(n, [0, 1, 0]);
            if self.foothold(up) && self.clear(offset(cell, [0, 2, 0])) {
                out.push(up);
                continue;
            }
            if self.clear(n) && self.clear(up) {
                for depth in 1..=DROP {
                    let to = offset(n, [0, -depth, 0]);
                    if self.foothold(to) {
                        out.push(to);
                        break;
                    }
                    if !self.clear(to) {
                        break;
                    }
                }
            }
        }
        out
    }

    fn sources(&self, cell: [i32; 3]) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        for side in SIDES {
            for dy in -1..=DROP {
                let from = offset(cell, [side[0], dy, side[2]]);
                if self.foothold(from) && self.moves(from).contains(&cell) {
                    out.push(from);
                }
            }
        }
        out
    }

    pub fn route(&self, from: [i32; 3], to: [i32; 3], nodes: u32) -> Route {
        if from == to {
            return Route::Open;
        }
        let mut seen: HashSet<[i32; 3]> = [from].into_iter().collect();
        let mut queue = VecDeque::from([from]);
        let mut expanded = 0;
        while let Some(cell) = queue.pop_front() {
            expanded += 1;
            if expanded > nodes {
                return Route::Undecided;
            }
            for next in self.moves(cell) {
                if next == to {
                    return Route::Open;
                }
                if seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        Route::Closed
    }

    pub fn flood(
        &self,
        from: [i32; 3],
        (min, max): ([i32; 3], [i32; 3]),
        toward: bool,
        nodes: u32,
    ) -> Option<Vec<([i32; 3], u32)>> {
        let inside = |c: [i32; 3]| (0..3).all(|i| (min[i]..=max[i]).contains(&c[i]));
        let mut moves: HashMap<[i32; 3], u32> = [(from, 0)].into_iter().collect();
        let mut reached = vec![(from, 0)];
        let mut queue = VecDeque::from([from]);
        while let Some(cell) = queue.pop_front() {
            let at = moves[&cell];
            let next = if toward {
                self.sources(cell)
            } else {
                self.moves(cell)
            };
            for n in next {
                if inside(n) && !moves.contains_key(&n) {
                    moves.insert(n, at + 1);
                    reached.push((n, at + 1));
                    queue.push_back(n);
                    if reached.len() > nodes as usize {
                        return None;
                    }
                }
            }
        }
        Some(reached)
    }

    pub fn aim(
        &self,
        feet: [f64; 3],
        pos: [i32; 3],
        record: Option<&BlockRecord>,
    ) -> Result<[f64; 3], ActionRefusal> {
        let Some(block) = self.state.block_at(pos) else {
            return Err(ActionRefusal::Unloaded);
        };
        let eye = [feet[0], feet[1] + EYE, feet[2]];
        if reach(eye, pos) > REACH {
            return Err(ActionRefusal::OutOfReach);
        }
        match record {
            None if self.state.row(block).info.replaceable => {
                return Err(ActionRefusal::NothingToDo)
            }
            Some(_) if !self.faced(pos) => return Err(ActionRefusal::NoFace),
            _ => {}
        }
        let centre = crate::geometry::centre_of(pos);
        if !self.sees(eye, centre, pos) {
            return Err(ActionRefusal::NoLineOfSight);
        }
        Ok(centre)
    }

    fn faced(&self, pos: [i32; 3]) -> bool {
        FACES.iter().any(|f| {
            self.state
                .block_at(offset(pos, *f))
                .is_some_and(|b| !self.state.row(b).info.replaceable)
        })
    }

    fn sees(&self, eye: [f64; 3], to: [f64; 3], target: [i32; 3]) -> bool {
        let steps = 64;
        (1..steps).all(|k| {
            let t = f64::from(k) / f64::from(steps);
            let at = std::array::from_fn(|i| eye[i] + (to[i] - eye[i]) * t);
            let cell = crate::geometry::cell_of(at);
            cell == target
                || self
                    .state
                    .block_at(cell)
                    .is_none_or(|b| self.state.row(b).info.collision.is_empty())
        })
    }
}

fn reach(eye: [f64; 3], cell: [i32; 3]) -> f64 {
    (0..3)
        .map(|i| {
            let lo = f64::from(cell[i]);
            let d = if eye[i] < lo {
                lo - eye[i]
            } else if eye[i] > lo + 1.0 {
                eye[i] - lo - 1.0
            } else {
                0.0
            };
            d * d
        })
        .sum::<f64>()
        .sqrt()
}
