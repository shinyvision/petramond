//! What the ground around the golem costs to move through: one cell grid of
//! open/floor/door/dig-time, and the cheapest weighted way across it. Both the
//! way OUT of a trap and the way IN to work walled off by ground are the same
//! question asked with different goals, so they share the model.

use crate::fx::{HashMap, HashSet};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use mod_sdk::*;

use super::tuning::price::{
    DESIGN_MOVES, DOOR_MOVES, FRUITLESS, HAND_SECONDS, HURT_MOVES, LEVEL_MOVES, MOVE_TICKS,
    SAFE_FALL,
};
use super::{Body, Ctx};
use crate::design::paged;
use crate::geometry::{offset, SIDES};
use crate::jobs::Job;
use crate::project::Project;

#[derive(Clone, Copy, Debug)]
pub(super) enum Move {
    /// Walk on (along, a step up, or a step down).
    Walk([i32; 3]),
    /// Off an edge, down onto the cell.
    Drop([i32; 3]),
    /// Up a level on a scaffold laid in the cell left.
    Rise,
    /// Dig out the floor underfoot and fall.
    Sink,
}

/// The first move of a way out: the cell it leaves from, the move, the cells
/// it digs, and the door it passes (lower half).
#[derive(Clone, Debug)]
pub(super) struct Way {
    pub from: [i32; 3],
    pub step: Move,
    pub digs: Vec<[i32; 3]>,
    pub door: Option<[i32; 3]>,
    pub cost: u32,
}

pub(super) type Clearing = (u32, Vec<[i32; 3]>, Option<[i32; 3]>);

/// What a cell of the search box holds for getting through it.
#[derive(Clone, Copy)]
pub(super) struct Cell {
    pub open: bool,
    /// Moves digging it costs; `None` when it must not or cannot be dug.
    pub dig: Option<u32>,
    /// Whether a body stands on it.
    pub floor: bool,
    /// A door the golem may open as the way.
    pub door: bool,
}

/// The cheapest way from where the golem walks to ground that walks home.
#[allow(clippy::too_many_arguments)]
pub(super) fn ground(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    lo: [i32; 3],
    size: [i32; 3],
    // Whether the design's blocks are left alone: a golem getting out digs
    // through its build and lays it again, one digging its way in must not.
    // Cells the design wants CLEAR are dug either way.
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
    // Cells already holding their design block, by the WORLD's measure, not
    // what this golem remembers laying: after a reload the build stands but
    // none of it is remembered, and a way in would cut through the player's
    // floor.
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
            // Never the table or anything else used by hand (a chest), nor a
            // door (it opens), nor what no tool breaks, nor a fluid.
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
            // A design block already in place, dug and relaid only when getting
            // out (see `spare_design`); a design cell holding anything else is
            // work to clear.
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

/// Whether every section of the site has been loaded long enough for a way
/// home judged shut to be believed. Checked only while it reads shut.
pub(super) fn search(
    grid: &HashMap<[i32; 3], Cell>,
    here: &HashMap<[i32; 3], u32>,
    home: &HashSet<[i32; 3]>,
    health: f32,
    failed: &[([i32; 3], [i32; 3])],
    rise: bool,
    // Ways down that cannot be walked back up: a golem getting out takes
    // them, one digging its way in would only shut itself in deeper.
    fall: bool,
) -> Option<Way> {
    let at = |c: [i32; 3]| grid.get(&c).copied();
    let up = |c: [i32; 3], n: i32| offset(c, [0, n, 0]);
    let door_of = |c: [i32; 3]| {
        at(c).filter(|cell| cell.door).map(|_| {
            if at(up(c, -1)).is_some_and(|below| below.door) {
                up(c, -1)
            } else {
                c
            }
        })
    };
    let clear = |cells: &[[i32; 3]]| -> Option<Clearing> {
        let mut cost = 0;
        let mut digs = Vec::new();
        let mut door = None;
        for c in cells {
            let cell = at(*c)?;
            if cell.open {
                continue;
            }
            if let Some(d) = door_of(*c) {
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
    };
    // Where a body falling from above `from` comes to rest, and how far it
    // fell; `None` past the box or onto something that is no floor.
    let landing = |from: [i32; 3], start: [i32; 3]| -> Option<([i32; 3], i32)> {
        let mut c = start;
        loop {
            let below = at(up(c, -1))?;
            if below.floor {
                return Some((c, from[1] - c[1]));
            }
            if !below.open {
                return None;
            }
            c = up(c, -1);
        }
    };
    // What a fall costs in moves, or `None` when it kills.
    let fall_cost = |fall: i32| {
        let hurt = (fall - SAFE_FALL).max(0);
        ((hurt as f32) < health).then(|| fall as u32 + HURT_MOVES * hurt as u32)
    };

    let mut best: HashMap<[i32; 3], u32> = HashMap::default();
    let mut came: HashMap<[i32; 3], Way> = HashMap::default();
    let mut queue = BinaryHeap::new();
    for (cell, moves) in here {
        if grid.contains_key(cell) {
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
        // Standing in a doorway, the panel may stand in the way out.
        let leaving = door_of(c).or_else(|| door_of(up(c, 1)));
        let mut moves = Vec::new();
        for side in SIDES {
            let n = offset(c, side);
            // Along, onto the floor there, or off the edge.
            if let Some((cost, digs, door)) = clear(&[up(n, 1), n]) {
                match at(up(n, -1)) {
                    Some(below) if below.floor => {
                        moves.push((n, Move::Walk(n), 1 + cost, digs, door))
                    }
                    Some(below) if below.open => {
                        if let Some((land, drop)) = landing(c, up(n, -1)) {
                            if let Some(hurt) = fall_cost(drop) {
                                let step = if drop <= 1 {
                                    Move::Walk(land)
                                } else {
                                    Move::Drop(land)
                                };
                                if fall || drop <= 1 {
                                    moves.push((land, step, 1 + cost + hurt, digs, door));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            // A step up onto the block beside: a staircase mined upward.
            if at(n).is_some_and(|b| b.floor && !b.door) {
                let to = up(n, 1);
                if let Some((cost, digs, door)) = clear(&[up(c, 2), up(to, 1), to]) {
                    moves.push((to, Move::Walk(to), 1 + cost, digs, door));
                }
            }
            // A step down beside. Digging a way in clears the head room over it
            // too, or the step cannot be walked back up.
            let to = up(n, -1);
            if at(up(to, -1)).is_some_and(|b| b.floor) {
                let needs: Vec<[i32; 3]> = if fall {
                    vec![n, to]
                } else {
                    vec![up(n, 1), n, to]
                };
                if let Some((cost, digs, door)) = clear(&needs) {
                    if !digs.is_empty() {
                        moves.push((to, Move::Walk(to), 1 + cost, digs, door));
                    }
                }
            }
        }
        if rise {
            if let Some((cost, digs, door)) = clear(&[up(c, 2)]) {
                moves.push((up(c, 1), Move::Rise, LEVEL_MOVES as u32 + cost, digs, door));
            }
        }
        if let Some(floor) = at(up(c, -1)).filter(|_| fall) {
            if let (Some(dig), Some((land, fall))) = (floor.dig, landing(c, up(c, -1))) {
                if let Some(hurt) = fall_cost(fall) {
                    moves.push((land, Move::Sink, dig + hurt, vec![up(c, -1)], None));
                }
            }
        }
        for (to, step, extra, digs, door) in moves {
            if !grid.contains_key(&to) || failed.contains(&(c, to)) {
                continue;
            }
            // Out of a doorway the panel may be in the way: opened first.
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
    // Back to the first move out of where the golem walks.
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
    // Ground that walks home is walked to already when nothing came before
    // it: the flood and the route disagree, and walking there settles it.
    Some(first.unwrap_or(Way {
        from: goal,
        step: Move::Walk(goal),
        digs: Vec::new(),
        door: None,
        cost: total,
    }))
}
