//! Getting unstuck. A golem on the ground that no route leads home from makes
//! its own way out, as a player would. Back onto its own pillar and down it if
//! one stands nearby; otherwise one search weighs every way out by what it
//! costs: walking; opening a shut door; digging, by how long the block takes
//! with the tools carried and dearer for the design's own blocks (they go back
//! up afterwards); dropping, dearer by the damage the fall does and never a
//! deadly one; mining down; and, only where nothing else gets out, pillaring
//! up. A golem that finds no way out, or makes no headway along one, sinks into
//! the ground and rises again at home, carrying what it carried.

use mod_sdk::*;

use super::ground::{ground, search, Move, Way};
use super::plan::walk_to;
use super::route::{self, Hubs};
use super::tuning::body::{HOP_LAUNCH, HOP_SPEED, PERCH_OFF_CENTRE, RISE_JUMP};
use super::tuning::hands::SWING;
use super::tuning::patience::{
    DOOR_TRIES, HOP_TICKS, RISE_TICKS, WAY_OUT_PATIENCE, WAY_OUT_PLANS, WAY_OUT_TOTAL_PLANS,
};
use super::tuning::reach::{OUT_ABOVE, OUT_AROUND, OUT_BELOW, PILLAR_BACK};
use super::tuning::route::SITE_SETTLE_TICKS;
use super::waiting::Waiting;
use super::wayin::open;
use super::{Body, Ctx, Step, Task, Then};
use crate::fx::{HashMap, HashSet};
use crate::geometry::{feet_of, manhattan, offset};
use crate::jobs::Job;
use crate::project::{Project, Projects};
/// A golem getting out of somewhere.
#[derive(Clone, Debug, Default)]
pub struct Stuck {
    at: [i32; 3],
    since: u64,
    plans: u32,
    /// Plans made getting out since it began, wherever they were made.
    total: u32,
    /// The top of the golem's own pillar it walked to, to climb down.
    pillar: Option<[i32; 3]>,
    /// Doors toggled as the way out, and how often.
    doors: Vec<([i32; 3], u8)>,
    /// Walks out that did not get there, by where they left from.
    failed: Vec<([i32; 3], [i32; 3])>,
}

impl Stuck {
    /// A block dug on the way out: headway, however long the digging took.
    pub fn dug(&mut self, now: u64) {
        self.since = now;
        self.plans = 0;
    }

    /// A walk out from `from` never reached `to`: that way is not walked.
    pub fn walk_failed(&mut self, from: [i32; 3], to: [i32; 3]) {
        if !self.failed.contains(&(from, to)) {
            self.failed.push((from, to));
        }
    }

    fn toggled(&mut self, door: [i32; 3]) {
        match self.doors.iter_mut().find(|(d, _)| *d == door) {
            Some((_, n)) => *n += 1,
            None => self.doors.push((door, 1)),
        }
    }
}

/// What looking for a way out found.
enum Lookout {
    Way(Way),
    NoWay,
    /// Part of the ground around is still loading: nothing about it is known.
    Loading,
}

/// The next step out for a golem no route leads home from.
pub fn rescue(ctx: &mut Ctx, job: &mut Job, body: &Body, project: &Project) -> Step {
    let stuck = job.crew.rescue.stuck.get_or_insert_with(Stuck::default);
    if stuck.at != body.cell || stuck.since == 0 {
        stuck.at = body.cell;
        stuck.since = ctx.now;
        stuck.plans = 0;
    }
    stuck.plans += 1;
    stuck.total += 1;
    if ctx.now > stuck.since + WAY_OUT_PATIENCE
        || stuck.plans > WAY_OUT_PLANS
        || stuck.total > WAY_OUT_TOTAL_PLANS
    {
        trace!(
            "TRACE stuck at {:?} for {} ticks: coming up at home",
            body.cell,
            ctx.now - stuck.since
        );
        return relocate(job);
    }
    // On the top of its own pillar: down it. Other scaffolding underfoot is
    // left alone while getting out, since one laid to rise out of a hole is no
    // pillar.
    if stuck.pillar == Some(body.cell) {
        if let Some(pillar) = super::pillar::recover(ctx, project, body) {
            job.crew.rescue.stuck = None;
            job.crew.aloft.perch = Some(pillar);
            return Step::Descend { since: ctx.now };
        }
    }
    let stuck = stuck.clone();
    job.crew.note = "The golem is finding its way out".into();
    match pillar_back(ctx, project, body) {
        None => return Step::Plan,
        Some(Some(top)) => {
            trace!(
                "TRACE stuck at {:?}: back to the pillar at {top:?}",
                body.cell
            );
            if let Some(stuck) = job.crew.rescue.stuck.as_mut() {
                stuck.pillar = Some(top);
            }
            return walk_to(ctx, top, Then::Escape);
        }
        Some(None) => {}
    }
    let mut here: HashMap<[i32; 3], u32> = match route::region(ctx, body.cell, false, &[]) {
        None => return Step::Plan,
        Some(Some(region)) => region
            .cells()
            .filter_map(|c| Some((c, region.moves(c)?)))
            .collect(),
        Some(None) => HashMap::default(),
    };
    here.retain(|c, _| !stuck.failed.contains(&(body.cell, *c)));
    here.insert(body.cell, 0);
    let home: HashSet<[i32; 3]> = match route::region(ctx, project.home, true, &[]) {
        None => return Step::Plan,
        Some(Some(region)) => region
            .cells()
            .filter(|c| {
                (c[0] - body.cell[0]).abs() <= OUT_AROUND
                    && (c[2] - body.cell[2]).abs() <= OUT_AROUND
                    && (body.cell[1] - OUT_BELOW..=body.cell[1] + OUT_ABOVE).contains(&c[1])
            })
            .collect(),
        Some(None) => HashSet::default(),
    };
    let health = mob_info(body.id).map_or(1.0, |m| m.health);
    let way = match way_out(ctx, job, project, body, &here, &home, health, &stuck) {
        Lookout::Way(way) => way,
        // A way judged with part of the ground unknown is a guess: a door not
        // in yet left digging the floor as the only way out.
        Lookout::Loading => {
            if let Some(stuck) = job.crew.rescue.stuck.as_mut() {
                stuck.since = ctx.now;
                stuck.plans = 0;
                stuck.total = stuck.total.saturating_sub(1);
            }
            job.crew.why(Waiting::GroundLoading);
            return Step::Plan;
        }
        Lookout::NoWay => {
            trace!(
                "TRACE stuck at {:?}: no way out within reach, coming up at home",
                body.cell
            );
            return relocate(job);
        }
    };
    trace!(
        "TRACE stuck at {:?}: way out costs {} moves, first {:?} from {:?} digging {:?} through door {:?}",
        body.cell, way.cost, way.step, way.from, way.digs, way.door
    );
    if way.from != body.cell {
        return walk_to(ctx, way.from, Then::Escape);
    }
    if let Some(cell) = way.digs.iter().copied().find(|c| !open(ctx, *c)) {
        return Step::Centre {
            task: Task::Breakout(cell),
            since: ctx.now,
        };
    }
    if let Some(door) = way.door {
        // A door already standing open lets the golem through: only a shut
        // one is toggled.
        let through = match way.step {
            Move::Walk(to) | Move::Drop(to) => route::probe(ctx, body.cell, to, Vec::new()),
            Move::Rise | Move::Sink => Some(Route::Closed),
        };
        match through {
            None => return Step::Plan,
            Some(Route::Open) => {}
            Some(_) => {
                if let Some(stuck) = job.crew.rescue.stuck.as_mut() {
                    stuck.toggled(door);
                }
                return Step::Use {
                    door,
                    open: true,
                    since: ctx.now,
                };
            }
        }
    }
    match way.step {
        Move::Walk(to) => walk_to(ctx, to, Then::Escape),
        Move::Drop(to) => {
            job.crew.trail.broken();
            Step::Hop { to, since: ctx.now }
        }
        Move::Rise => Step::Rise {
            from: body.cell,
            since: ctx.now,
        },
        // Dug out already and still standing: the next plan looks again.
        Move::Sink => Step::Plan,
    }
}

/// Sink into the ground here and rise again at home.
pub fn relocate(job: &mut Job) -> Step {
    job.crew.rescue.stuck = None;
    Step::Relocate { t: 0, leg: 0 }
}

#[allow(clippy::too_many_arguments)]
fn way_out(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    here: &HashMap<[i32; 3], u32>,
    home: &HashSet<[i32; 3]>,
    health: f32,
    stuck: &Stuck,
) -> Lookout {
    let lo = offset(body.cell, [-OUT_AROUND, -OUT_BELOW, -OUT_AROUND]);
    let size = [
        2 * OUT_AROUND + 1,
        OUT_BELOW + OUT_ABOVE + 1,
        2 * OUT_AROUND + 1,
    ];
    let mut grid = match ground(ctx, job, project, body, lo, size, false) {
        Some(grid) => grid,
        None => return Lookout::Loading,
    };
    // A door toggled as the way out often enough without freeing the golem is
    // a wall like any other.
    for (door, tries) in &stuck.doors {
        if *tries >= DOOR_TRIES {
            for half in [*door, offset(*door, [0, 1, 0])] {
                if let Some(cell) = grid.get_mut(&half) {
                    cell.door = false;
                }
            }
        }
    }
    // A staircase, a door or a way dug through first: a scaffold to rise on
    // only where nothing else gets out.
    match search(&grid, here, home, health, &stuck.failed, false, true)
        .or_else(|| search(&grid, here, home, health, &stuck.failed, true, true))
    {
        Some(way) => Lookout::Way(way),
        None => Lookout::NoWay,
    }
}

/// What every cell of a box is to a golem moving through it: open, a floor,
/// a door, and what digging it costs with the tools it carries. `None` while
/// any of the ground is still loading — nothing about it is known then.
pub fn site_settled(ctx: &mut Ctx, job: &mut Job) -> bool {
    let (min, max) = ctx.site;
    let steps = |lo: i32, hi: i32| (lo..=hi).step_by(16).chain(std::iter::once(hi));
    let loaded = steps(min[0], max[0]).all(|x| {
        steps(min[1], max[1]).all(|y| steps(min[2], max[2]).all(|z| is_loaded([x, y, z])))
    });
    if !loaded {
        job.crew.rescue.site_loaded_since = None;
        return false;
    }
    match job.crew.rescue.site_loaded_since {
        Some(since) => ctx.now >= since + SITE_SETTLE_TICKS,
        None => {
            job.crew.rescue.site_loaded_since = Some(ctx.now);
            ctx.routes.clear();
            ctx.regions.clear();
            false
        }
    }
}

/// The cheapest way through `grid` from where the golem walks to ground that
/// walks home, rising on scaffolds or not.
pub fn rise(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    from: [i32; 3],
    since: u64,
) -> Step {
    let id = body.id;
    job.crew.presence.set_goal(id, None);
    job.crew.presence.set_hold(id, true);
    job.crew.presence.look_at(
        id,
        ctx.now,
        body.pos,
        [
            f64::from(from[0]) + 0.5,
            body.pos[1] - 1.0,
            f64::from(from[2]) + 0.5,
        ],
    );
    if ctx.now > since + RISE_TICKS {
        return Step::Plan;
    }
    if body.on_ground {
        if body.cell[1] > from[1] {
            return Step::Plan;
        }
        let [dx, dz] = body.off_centre();
        if dx.abs() > PERCH_OFF_CENTRE || dz.abs() > PERCH_OFF_CENTRE {
            super::centre(body);
            return Step::Rise { from, since };
        }
        if ctx.now >= job.crew.presence.gaze_since + SWING {
            super::legs::jump(body, RISE_JUMP);
        }
        return Step::Rise { from, since };
    }
    super::hold_still(body);
    if body.pos[1] >= f64::from(from[1]) + 1.02
        && open(ctx, from)
        && super::scaffold::lay(ctx, job, body, from) == super::hands::Lay::Queued
    {
        projects.update(job.id, |p| {
            if !p.scaffolds.contains(&from) {
                p.scaffolds.push(from);
            }
        });
    }
    Step::Rise { from, since }
}

/// The top of one of the golem's own scaffold pillars nearby, standing on
/// something all the way down, that it walks onto from here. `None` = no
/// route budget this tick.
pub fn pillar_back(ctx: &mut Ctx, project: &Project, body: &Body) -> Option<Option<[i32; 3]>> {
    let mut tops: Vec<[i32; 3]> = project
        .scaffolds
        .iter()
        .map(|c| offset(*c, [0, 1, 0]))
        .filter(|t| !project.scaffolds.contains(t) && manhattan(*t, body.cell) <= PILLAR_BACK)
        .filter(|t| t[1] > body.cell[1] - 3 && *t != body.cell)
        .collect();
    tops.sort_by_key(|t| (manhattan(*t, body.cell), *t));
    for top in tops.into_iter().take(6) {
        let mut base = top[1] - 1;
        while project.scaffolds.contains(&[top[0], base - 1, top[2]]) {
            base -= 1;
        }
        // A column of at least two, on something: a lone step is no pillar.
        if top[1] - base < 2 || !open(ctx, top) || open(ctx, [top[0], base - 1, top[2]]) {
            continue;
        }
        if route::probe(ctx, body.cell, top, Vec::new())? == Route::Open {
            return Some(Some(top));
        }
    }
    Some(None)
}

/// The hop itself: one launch toward the landing, steered over it through
/// the rise as a player steers a jump; the fall carries it down.
pub fn hop(job: &mut Job, body: &Body, to: [i32; 3], since: u64, now: u64) -> Step {
    job.crew.presence.set_hold(body.id, true);
    let centre = feet_of(to);
    let (dx, dz) = (centre[0] - body.pos[0], centre[2] - body.pos[2]);
    let v = |d: f64| ((d * 2.5) as f32).clamp(-HOP_SPEED, HOP_SPEED);
    if now == since + 1 || (now <= since + 3 && body.on_ground) {
        super::legs::launch(body, [v(dx), HOP_LAUNCH, v(dz)]);
        return Step::Hop { to, since };
    }
    if !body.on_ground {
        super::legs::steer(body, [v(dx), v(dz)]);
    }
    if body.on_ground || now > since + HOP_TICKS {
        return Step::Plan;
    }
    Step::Hop { to, since }
}

/// Whether the golem stands somewhere no route leads home from.
pub fn stranded(ctx: &mut Ctx, hubs: Hubs, body: &Body) -> bool {
    matches!(route::out(ctx, hubs, body.cell, &[]), Some(Route::Closed))
}
