use crate::fx::{HashMap, HashSet};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::host::prelude::*;

use super::tuning::price::{
    DESIGN_MOVES, DOOR_MOVES, FRUITLESS, HAND_SECONDS, HURT_MOVES, LEVEL_MOVES, MOVE_TICKS,
    SAFE_FALL,
};
use super::{Body, Ctx};
use crate::geometry::{offset, SIDES};
use crate::project::Project;
use crate::worker::Job;

#[derive(Clone, Copy, Debug)]
pub(super) enum Move {
    Walk([i32; 3]),
    Drop([i32; 3]),
    Rise,
    Sink,
}

#[derive(Clone, Debug)]
pub(super) struct Way {
    pub from: [i32; 3],
    pub step: Move,
    pub digs: Vec<[i32; 3]>,
    pub door: Option<[i32; 3]>,
    pub cost: u32,
}

pub(super) type Clearing = (u32, Vec<[i32; 3]>, Option<[i32; 3]>);

#[derive(Clone, Copy)]
pub(super) struct Cell {
    pub open: bool,
    pub dig: Option<u32>,
    pub floor: bool,
    pub door: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ground(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    lo: [i32; 3],
    size: [i32; 3],
    spare_design: bool,
) -> Option<HashMap<[i32; 3], Cell>> {
    let mut positions = Vec::with_capacity((size[0] * size[1] * size[2]) as usize);
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..size[2] {
                positions.push(offset(lo, [x, y, z]));
            }
        }
    }
    let blocks = paged(positions.clone(), get_blocks);
    let supplies = ctx.supplies.chain(project.table);
    let table = ctx.content.table;
    let mut tools: Vec<(String, u8, f32)> = Vec::new();
    for stack in body.slots.iter().flatten() {
        if let Some(tool) = ctx.caches.item(&stack.item).and_then(|i| i.tool.as_ref()) {
            tools.push((tool.kind.clone(), tool.tier, tool.speed));
        }
    }
    let built: HashSet<[i32; 3]> = match job.survey.as_ref() {
        Some(survey) => survey
            .known
            .iter()
            .enumerate()
            .filter(|(_, known)| matches!(known, crate::survey::Known::Satisfied))
            .flat_map(|(i, _)| job.design.cells(job.design.units[i]))
            .collect(),
        None => HashSet::default(),
    };
    let mut kinds: HashMap<BlockId, Cell> = HashMap::default();
    let mut grid: HashMap<[i32; 3], Cell> =
        HashMap::with_capacity_and_hasher(positions.len(), Default::default());
    let unknown = Cell {
        open: false,
        dig: None,
        floor: false,
        door: false,
    };
    if blocks.iter().any(Option::is_none) {
        return None;
    }
    for (pos, block) in positions.into_iter().zip(blocks) {
        let Some(block) = block else {
            continue;
        };
        let kind = *kinds.entry(block).or_insert_with(|| {
            let Some(info) = ctx.caches.block(block) else {
                return unknown;
            };
            let open = info.collision.is_empty() && info.fluid.is_none();
            let door = info.interaction == Some(BlockUse::ToggleDoor);
            let keep = block == table
                || info.interaction.is_some()
                || info.hardness < 0.0
                || info.fluid.is_some();
            let dig = (!open && !keep).then(|| {
                let hand = info.hardness * HAND_SECONDS * 20.0;
                let speed = info.preferred_tool.as_deref().and_then(|kind| {
                    tools
                        .iter()
                        .filter(|(k, tier, _)| k == kind && *tier >= info.harvest_tier.max(1))
                        .map(|(_, _, speed)| *speed)
                        .reduce(f32::max)
                });
                let ticks = match speed {
                    Some(speed) => hand / speed.max(1.0),
                    None if info.harvest_tier >= 1 => hand * FRUITLESS,
                    None => hand,
                };
                (ticks / MOVE_TICKS as f32).ceil().max(1.0) as u32
            });
            Cell {
                open,
                dig,
                floor: !open && info.fluid.is_none(),
                door,
            }
        });
        let cell = if supplies.contains(&pos) {
            Cell { dig: None, ..kind }
        } else if job.design.governed.contains(&pos) && (!spare_design || built.contains(&pos)) {
            Cell {
                dig: kind.dig.filter(|_| !spare_design).map(|d| d + DESIGN_MOVES),
                ..kind
            }
        } else {
            kind
        };
        grid.insert(pos, cell);
    }
    Some(grid)
}

#[derive(Clone, Copy)]
struct Grid<'a> {
    cells: &'a HashMap<[i32; 3], Cell>,
}

impl Grid<'_> {
    fn at(self, c: [i32; 3]) -> Option<Cell> {
        self.cells.get(&c).copied()
    }

    fn door_of(self, c: [i32; 3]) -> Option<[i32; 3]> {
        self.at(c).filter(|cell| cell.door).map(|_| {
            if self.at(up(c, -1)).is_some_and(|below| below.door) {
                up(c, -1)
            } else {
                c
            }
        })
    }

    fn clear(self, cells: &[[i32; 3]]) -> Option<Clearing> {
        let mut cost = 0;
        let mut digs = Vec::new();
        let mut door = None;
        for c in cells {
            let cell = self.at(*c)?;
            if cell.open {
                continue;
            }
            if let Some(d) = self.door_of(*c) {
                if door.is_none() {
                    cost += DOOR_MOVES;
                }
                door = Some(d);
                continue;
            }
            cost += cell.dig?;
            digs.push(*c);
        }
        Some((cost, digs, door))
    }

    fn landing(self, from: [i32; 3], start: [i32; 3]) -> Option<([i32; 3], i32)> {
        let mut c = start;
        loop {
            let below = self.at(up(c, -1))?;
            if below.floor {
                return Some((c, from[1] - c[1]));
            }
            if !below.open {
                return None;
            }
            c = up(c, -1);
        }
    }
}

fn up(c: [i32; 3], n: i32) -> [i32; 3] {
    offset(c, [0, n, 0])
}

pub(super) fn fall_cost(fall: i32, health: f32) -> Option<u32> {
    let hurt = (fall - SAFE_FALL).max(0);
    ((hurt as f32) < health).then(|| fall as u32 + HURT_MOVES * hurt as u32)
}

#[derive(Clone, Copy)]
struct Rules {
    health: f32,
    rise: bool,
    fall: bool,
}

type Onward = ([i32; 3], Move, u32, Vec<[i32; 3]>, Option<[i32; 3]>);

fn moves_from(grid: Grid, rules: Rules, c: [i32; 3]) -> Vec<Onward> {
    let mut moves = Vec::new();
    for side in SIDES {
        let n = offset(c, side);
        if let Some((cost, digs, door)) = grid.clear(&[up(n, 1), n]) {
            match grid.at(up(n, -1)) {
                Some(below) if below.floor => moves.push((n, Move::Walk(n), 1 + cost, digs, door)),
                Some(below) if below.open => {
                    if let Some((land, drop)) = grid.landing(c, up(n, -1)) {
                        if let Some(hurt) = fall_cost(drop, rules.health) {
                            let step = if drop <= 1 {
                                Move::Walk(land)
                            } else {
                                Move::Drop(land)
                            };
                            if rules.fall || drop <= 1 {
                                moves.push((land, step, 1 + cost + hurt, digs, door));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if grid.at(n).is_some_and(|b| b.floor && !b.door) {
            let to = up(n, 1);
            if let Some((cost, digs, door)) = grid.clear(&[up(c, 2), up(to, 1), to]) {
                moves.push((to, Move::Walk(to), 1 + cost, digs, door));
            }
        }
        let to = up(n, -1);
        if grid.at(up(to, -1)).is_some_and(|b| b.floor) {
            let needs: Vec<[i32; 3]> = if rules.fall {
                vec![n, to]
            } else {
                vec![up(n, 1), n, to]
            };
            if let Some((cost, digs, door)) = grid.clear(&needs) {
                if !digs.is_empty() {
                    moves.push((to, Move::Walk(to), 1 + cost, digs, door));
                }
            }
        }
    }
    if rules.rise {
        if let Some((cost, digs, door)) = grid.clear(&[up(c, 2)]) {
            moves.push((up(c, 1), Move::Rise, LEVEL_MOVES as u32 + cost, digs, door));
        }
    }
    if let Some(floor) = grid.at(up(c, -1)).filter(|_| rules.fall) {
        if let (Some(dig), Some((land, fall))) = (floor.dig, grid.landing(c, up(c, -1))) {
            if let Some(hurt) = fall_cost(fall, rules.health) {
                moves.push((land, Move::Sink, dig + hurt, vec![up(c, -1)], None));
            }
        }
    }
    moves
}

/// Cheapest path from `here` (walked-to cells, with cost) down to ground in `home`. Only the
/// first move comes back. Skips `failed` (from, to) moves. `rise` lets it climb scaffolds.
pub(super) fn search(
    grid: &HashMap<[i32; 3], Cell>,
    here: &HashMap<[i32; 3], u32>,
    home: &HashSet<[i32; 3]>,
    health: f32,
    failed: &[([i32; 3], [i32; 3])],
    rise: bool,
    fall: bool,
) -> Option<Way> {
    let rules = Rules { health, rise, fall };
    let grid = Grid { cells: grid };
    let mut best: HashMap<[i32; 3], u32> = HashMap::default();
    let mut came: HashMap<[i32; 3], Way> = HashMap::default();
    let mut queue = BinaryHeap::new();
    for (cell, moves) in here {
        if grid.cells.contains_key(cell) {
            best.insert(*cell, *moves);
            queue.push(Reverse((*moves, *cell)));
        }
    }
    let mut goal = None;
    while let Some(Reverse((cost, c))) = queue.pop() {
        if best.get(&c).is_some_and(|b| *b < cost) {
            continue;
        }
        if home.contains(&c) {
            goal = Some((c, cost));
            break;
        }
        let leaving = grid.door_of(c).or_else(|| grid.door_of(up(c, 1)));
        for (to, step, extra, digs, door) in moves_from(grid, rules, c) {
            if !grid.cells.contains_key(&to) || failed.contains(&(c, to)) {
                continue;
            }
            let (extra, door) = match (door, leaving) {
                (Some(d), _) => (extra, Some(d)),
                (None, Some(l)) if !matches!(step, Move::Sink) => (extra + DOOR_MOVES, Some(l)),
                _ => (extra, None),
            };
            let next = cost + extra;
            if best.get(&to).is_none_or(|b| next < *b) {
                best.insert(to, next);
                came.insert(
                    to,
                    Way {
                        from: c,
                        step,
                        digs,
                        door,
                        cost: 0,
                    },
                );
                queue.push(Reverse((next, to)));
            }
        }
    }
    let (goal, total) = goal?;
    Some(first_move(&came, here, goal, total))
}

fn first_move(
    came: &HashMap<[i32; 3], Way>,
    here: &HashMap<[i32; 3], u32>,
    goal: [i32; 3],
    total: u32,
) -> Way {
    let mut at_cell = goal;
    let mut first = None;
    while let Some(way) = came.get(&at_cell) {
        first = Some(Way {
            from: way.from,
            step: way.step,
            digs: way.digs.clone(),
            door: way.door,
            cost: total,
        });
        if here.contains_key(&way.from) && !came.contains_key(&way.from) {
            break;
        }
        at_cell = way.from;
    }
    first.unwrap_or(Way {
        from: goal,
        step: Move::Walk(goal),
        digs: Vec::new(),
        door: None,
        cost: total,
    })
}
