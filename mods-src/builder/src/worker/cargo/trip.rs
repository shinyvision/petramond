//! Trips to the table's chests: when one is worth making, and the walk there.

use mod_sdk::*;

use super::wants::{digs_ahead, keeps, tool_kind, wanted};
use crate::content::BLUEPRINT;
use crate::geometry::{feet_of, manhattan, reach_to, reaches};
use crate::jobs::Job;
use crate::project::{Hold, Project, Projects};
use crate::survey::{key_of, ItemKey};
use crate::worker::plan::Mode;
use crate::worker::route::{self, Hubs};
use crate::worker::stance::{self, Search};
use crate::worker::tuning::reach::CHEST_REACH;
use crate::worker::waiting::Waiting;
use crate::worker::{sight, Body, Ctx, Step, Then, TRACE};

/// Head for the chest holding what the work ahead needs, or hold the job
/// for supplies nothing connected holds once no other work is waiting.
pub fn resupply(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    project: &Project,
    waiting: bool,
) -> Step {
    let stock = ctx.supplies.stock(project.table);
    // What the chests hold is unknown while one is still streaming in; planning
    // against a guess sends the golem off for blocks that are not there.
    if !stock.read {
        job.crew.why(Waiting::ChestsUnread);
        return Step::Plan;
    }
    let (need, kinds) = wanted(ctx, job, &body.slots, stock.read.then_some(&stock.totals));
    let tool_in_stock = |ctx: &mut Ctx| {
        !kinds.is_empty()
            && stock
                .slots
                .iter()
                .flatten()
                .flatten()
                .any(|s| tool_kind(ctx, &s.item).is_some_and(|(k, _)| kinds.contains(&k)))
    };
    // Nothing the chests hold is wanted: say what the work is short of and
    // wait.
    if need.is_empty() && !tool_in_stock(ctx) {
        short_of_bill(ctx, projects, job, &stock, waiting);
        return Step::Plan;
    }
    let useful = stock
        .containers
        .iter()
        .zip(&stock.slots)
        .filter(|(_, slots)| {
            slots.iter().flatten().any(|s| {
                need.contains_key(&key_of(s))
                    || tool_kind(ctx, &s.item).is_some_and(|(k, _)| kinds.contains(&k))
            })
        })
        .map(|(c, _)| *c)
        .min_by_key(|c| manhattan(*c, body.cell));
    let Some(container) = useful else {
        short_of_bill(ctx, projects, job, &stock, waiting);
        return Step::Plan;
    };
    if TRACE {
        let load: Vec<String> = need
            .iter()
            .map(|((item, _), n)| format!("{n}x{}", item.trim_start_matches("petramond:")))
            .collect();
        log(&format!(
            "TRACE a trip to {container:?} for {load:?} and tools {kinds:?} (hands hold {})",
            body.slots.iter().flatten().count()
        ));
    }
    go_to_container(
        ctx,
        job,
        body,
        project.home,
        container,
        Then::Fetch(container),
    )
}

/// What the work still standing wants beyond everything connected: said on
/// the table, and waited for when the golem has nothing else to get on with.
pub(super) fn short_of_bill(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &Job,
    stock: &crate::supplies::Stock,
    waiting: bool,
) {
    // Chests nobody can read say nothing about what is missing.
    if !stock.read {
        return;
    }
    let Some(summary) = job.summary() else {
        return;
    };
    let short = crate::supplies::shortfall(&summary.bill, &stock.totals);
    if short.is_empty() {
        // Stocked again: the table says so no longer.
        projects.update(job.id, |p| {
            if p.hold().is_none() {
                p.note.clear();
            }
        });
        return;
    }
    short_of(projects, job, &short, ctx.caches, !waiting);
}

/// Say on the table what the work wants and nothing connected holds. With a
/// `hold` the golem waits for the owner to stock it; without one it builds what
/// else it can.
fn short_of(
    projects: &mut Projects,
    job: &Job,
    missing: &[(ItemKey, u32)],
    names: &mut crate::caches::Caches,
    hold: bool,
) {
    let text = match missing.first() {
        Some(((item, _), count)) => format!(
            "Missing {count}x {}{}",
            names.display_name(item),
            if missing.len() > 1 { " and more" } else { "" }
        ),
        None => String::new(),
    };
    projects.update(job.id, |p| {
        if hold {
            p.hold_for(Hold::Supplies, text);
        } else {
            p.note = text;
        }
    });
}

/// Whether the golem can work a container from where it stands, judged from its
/// CELL as the trip's plan judged the stance: judged from the exact body
/// position, a stance could pass the plan and fail on arrival.
pub(super) fn at_container(body: &Body, container: [i32; 3]) -> bool {
    reaches(feet_of(body.cell), &[container]) || reaches(body.pos, &[container])
}

pub(super) fn go_to_container(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    home: [i32; 3],
    container: [i32; 3],
    then: Then,
) -> Step {
    if at_container(body, container) {
        return match then {
            Then::Fetch(_) | Then::Deposit(_) => Step::Walk {
                to: body.cell,
                goal: body.cell,
                then,
                best: 0,
                progress: ctx.now,
            },
            _ => Step::Plan,
        };
    }
    let trail = job.crew.trail.clone();
    let hubs = Hubs::new(home, &trail);
    let close = |s: [i32; 3]| reach_to(feet_of(s), container) <= CHEST_REACH;
    match stance::find(
        ctx,
        body,
        hubs,
        &[container],
        &sight::Work::Touch,
        false,
        close,
    ) {
        Search::Found(to) => match route::leg(ctx, hubs, body, to) {
            Some(Some(cell)) => Step::Walk {
                to: cell,
                goal: to,
                then,
                best: i32::MAX,
                progress: ctx.now,
            },
            _ => Step::Plan,
        },
        Search::Busy => Step::Plan,
        // Nothing near the chests routes from here: head back toward where
        // the golem emerged, which the work never cuts off, and look again.
        Search::None | Search::Unseen if body.cell != home => Step::Walk {
            to: home,
            goal: home,
            then: Then::Regroup,
            best: i32::MAX,
            progress: ctx.now,
        },
        Search::None | Search::Unseen => {
            job.crew.note = "The golem cannot reach the chests".into();
            Step::Plan
        }
    }
}

/// With no room left in the golem's hands, take back what nothing ahead
/// needs, or make room for what digging still to come collects.
pub fn unload(ctx: &mut Ctx, job: &mut Job, body: &Body, project: &Project) -> Option<Step> {
    let junk = body
        .slots
        .iter()
        .flatten()
        .any(|s| !keeps(ctx, job, &body.slots, s));
    if !junk && !digs_ahead(job) {
        return None;
    }
    unload_all(ctx, job, body, project)
}

pub fn unload_all(ctx: &mut Ctx, job: &mut Job, body: &Body, project: &Project) -> Option<Step> {
    // The nearest chest with a slot free, before a full one nearer.
    let stock = ctx.supplies.stock(project.table);
    let nearest = stock
        .containers
        .iter()
        .zip(&stock.slots)
        .min_by_key(|(c, slots)| (slots.iter().all(Option::is_some), manhattan(**c, body.cell)))
        .map(|(c, _)| c)?;
    Some(go_to_container(
        ctx,
        job,
        body,
        project.home,
        *nearest,
        Then::Deposit(*nearest),
    ))
}

/// Whether a trip to the chests would put anything back: on the way home
/// everything carried, otherwise what nothing ahead needs.
pub fn returns_anything(ctx: &mut Ctx, job: &Job, body: &Body, project: &Project) -> bool {
    let everything = hands_in_everything(project);
    // Its plans stay with the golem to the end.
    body.slots
        .iter()
        .flatten()
        .filter(|stack| stack.item != BLUEPRINT)
        .any(|stack| everything || !keeps(ctx, job, &body.slots, stack))
}

/// A golem on its way home leaves for good (a job resumed summons another):
/// everything goes back, its tools too, rather than being set down in the
/// grass where it sinks.
pub(super) fn hands_in_everything(project: &Project) -> bool {
    Mode::of(project) != Mode::Building
}
