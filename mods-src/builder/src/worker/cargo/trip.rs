use crate::host::prelude::*;

use super::wants::{keeps, wanted};
use crate::content::BLUEPRINT;
use crate::geometry::{feet_of, manhattan, reach_to, reaches};
use crate::project::{Hold, Note, Project, Projects};
use crate::survey::ItemKey;
use crate::worker::plan::Mode;
use crate::worker::route::{self, Hubs};
use crate::worker::stance::{self, Search};
use crate::worker::tuning::reach::CHEST_REACH;
use crate::worker::waiting::Waiting;
use crate::worker::Job;
use crate::worker::{sight, Body, Ctx, Step, Then, TRACE};

pub fn resupply(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    project: &Project,
    waiting: bool,
) -> Step {
    let stock = ctx.supplies.stock(project.table);
    if !stock.read {
        job.crew.why(Waiting::ChestsUnread);
        return Step::Plan;
    }
    let load = wanted(ctx, job, &body.slots, &stock.totals);
    let Some(container) = load.container(ctx, body, &stock) else {
        short_of_bill(ctx, projects, job, &stock, waiting);
        return Step::Plan;
    };
    if TRACE {
        let items: Vec<String> = load
            .missing(&body.slots)
            .iter()
            .map(|((item, _), n)| format!("{n}x{}", item.trim_start_matches("petramond:")))
            .collect();
        log(&format!(
            "TRACE a trip to {container:?} for {items:?} and tools {:?} (hands hold {})",
            load.tools,
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

pub(super) fn short_of_bill(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &Job,
    stock: &crate::supplies::Stock,
    waiting: bool,
) {
    if !stock.read {
        return;
    }
    let Some(summary) = job.summary() else {
        return;
    };
    let short = crate::supplies::shortfall(&summary.bill, &stock.totals);
    if short.is_empty() {
        projects.update(job.id, |p| {
            if p.hold().is_none() {
                p.note = Note::None;
            }
        });
        return;
    }
    short_of(projects, job, &short, ctx.caches, !waiting);
}

fn short_of(
    projects: &mut Projects,
    job: &Job,
    missing: &[(ItemKey, u32)],
    names: &mut crate::caches::Caches,
    hold: bool,
) {
    let note = crate::supplies::worst(missing, names).map_or(Note::None, Note::Missing);
    projects.update(job.id, |p| {
        if hold {
            p.hold_for(Hold::Supplies, note);
        } else {
            p.note = note;
        }
    });
}

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
    match approach(ctx, job, body, home, container, then) {
        Search::Found(step) => step,
        Search::Busy => Step::Plan,
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

pub(super) fn approach(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    home: [i32; 3],
    container: [i32; 3],
    then: Then,
) -> Search<Step> {
    if at_container(body, container) {
        return Search::Found(Step::Walk {
            to: body.cell,
            goal: body.cell,
            then,
            best: 0,
            progress: ctx.now,
        });
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
            Some(Some(cell)) => Search::Found(Step::Walk {
                to: cell,
                goal: to,
                then,
                best: i32::MAX,
                progress: ctx.now,
            }),
            Some(None) => Search::Found(Step::Plan),
            None => Search::Busy,
        },
        Search::Busy => Search::Busy,
        Search::None | Search::Unseen => Search::None,
    }
}

pub fn unload_all(ctx: &mut Ctx, job: &mut Job, body: &Body, project: &Project) -> Option<Step> {
    let stock = ctx.supplies.stock(project.table);
    let nearest = deposit_container(&stock, body)?;
    Some(go_to_container(
        ctx,
        job,
        body,
        project.home,
        nearest,
        Then::Deposit(nearest),
    ))
}

pub(super) fn deposit_container(stock: &crate::supplies::Stock, body: &Body) -> Option<[i32; 3]> {
    stock
        .containers
        .iter()
        .zip(&stock.slots)
        .min_by_key(|(c, slots)| (slots.iter().all(Option::is_some), manhattan(**c, body.cell)))
        .map(|(c, _)| *c)
}

pub fn returns_anything(ctx: &mut Ctx, job: &Job, body: &Body, project: &Project) -> bool {
    let everything = hands_in_everything(project);
    body.slots
        .iter()
        .flatten()
        .filter(|stack| stack.item != BLUEPRINT)
        .any(|stack| everything || !keeps(ctx, job, &body.slots, stack))
}

pub(super) fn hands_in_everything(project: &Project) -> bool {
    Mode::of(project) != Mode::Building
}
