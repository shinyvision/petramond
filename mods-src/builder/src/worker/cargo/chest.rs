use std::collections::BTreeMap;

use crate::host::prelude::*;

use super::trip::{at_container, go_to_container, hands_in_everything, short_of_bill};
use super::wants::{dig_room, digs_ahead, keeps, needed_at, tool_kind, wanted};
use crate::content::BLUEPRINT;
use crate::project::{Hold, Note, Phase, Projects};
use crate::survey::key_of;
use crate::worker::tuning::hands::{LID_UP, LINGER};
use crate::worker::Job;
use crate::worker::{scaffold, Body, Ctx, Step, Then};

pub fn fetch(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    from: [i32; 3],
) -> Step {
    let Some(project) = projects.get(job.id).cloned() else {
        return Step::Plan;
    };
    let stock = ctx.supplies.stock(project.table);
    let (mut need, mut kinds) = wanted(ctx, job, &body.slots, stock.read.then_some(&stock.totals));
    let mut moved_any = false;
    let wanted_at_first = need.clone();
    let mut best_tools: BTreeMap<String, ([i32; 3], u32, f32)> = BTreeMap::new();
    for (container, slots) in stock.containers.iter().zip(&stock.slots) {
        if !at_container(body, *container) {
            continue;
        }
        for (slot, stack) in slots.iter().enumerate() {
            let Some(stack) = stack else { continue };
            if let Some((kind, speed)) = tool_kind(ctx, &stack.item) {
                if kinds.contains(&kind) && best_tools.get(&kind).is_none_or(|b| speed > b.2) {
                    best_tools.insert(kind, (*container, slot as u32, speed));
                }
                continue;
            }
            let key = key_of(stack);
            let Some(want) = need.get_mut(&key) else {
                continue;
            };
            let count = (*want).min(u32::from(stack.count)) as u8;
            if count == 0 {
                continue;
            }
            if let Some(moved) = container_transfer(
                ContainerAddress::Block(*container),
                slot as u32,
                ContainerAddress::Mob(body.id),
                count,
            ) {
                *want -= u32::from(moved.count);
                moved_any = true;
            }
        }
    }
    for (kind, (container, slot, _)) in best_tools {
        if container_transfer(
            ContainerAddress::Block(container),
            slot,
            ContainerAddress::Mob(body.id),
            1,
        )
        .is_some()
        {
            kinds.remove(&kind);
            moved_any = true;
        }
    }
    let tool_elsewhere = stock
        .containers
        .iter()
        .zip(&stock.slots)
        .filter(|(c, _)| !at_container(body, **c))
        .find(|(_, slots)| {
            slots
                .iter()
                .flatten()
                .any(|s| tool_kind(ctx, &s.item).is_some_and(|(k, _)| kinds.contains(&k)))
        })
        .map(|(c, _)| *c);
    if let Some(container) = tool_elsewhere {
        return go_to_container(
            ctx,
            job,
            body,
            project.home,
            container,
            Then::Fetch(container),
        );
    }
    if !moved_any {
        trace!(
            "TRACE the trip to {from:?} took nothing: wanted {:?} from {:?}",
            wanted_at_first
                .keys()
                .map(|(item, _)| item.clone())
                .collect::<Vec<_>>(),
            body.cell,
        );
        need.retain(|_, n| *n > 0);
        let elsewhere = stock
            .containers
            .iter()
            .zip(&stock.slots)
            .filter(|(c, _)| !at_container(body, **c))
            .find(|(_, slots)| {
                slots.iter().flatten().any(|s| {
                    need.contains_key(&key_of(s))
                        || tool_kind(ctx, &s.item).is_some_and(|(k, _)| kinds.contains(&k))
                })
            })
            .map(|(c, _)| *c);
        if let Some(container) = elsewhere {
            return go_to_container(
                ctx,
                job,
                body,
                project.home,
                container,
                Then::Fetch(container),
            );
        }
        if body.slots.iter().all(Option::is_some) {
            projects.update(job.id, |p| p.hold_for(Hold::Storage, Note::HandsFull));
        } else {
            let can_build = body.slots.iter().flatten().any(|stack| {
                tool_kind(ctx, &stack.item).is_none()
                    && !scaffold::keeps(ctx, job, &body.slots, stack)
                    && keeps(ctx, job, &body.slots, stack)
            });
            short_of_bill(ctx, projects, job, &stock, can_build);
        }
    }
    Step::Plan
}

pub fn open(ctx: &Ctx, job: &mut Job, body: &Body, container: [i32; 3], deposit: bool) -> Step {
    job.crew.presence.set_hold(body.id, true);
    job.crew.presence.face(body.id, Some((body.pos, container)));
    container_hold(ContainerAddress::Block(container), body.actor(), true);
    Step::Rummage {
        container,
        deposit,
        since: ctx.now,
        moved: false,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rummage(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    container: [i32; 3],
    deposit: bool,
    since: u64,
    moved: bool,
) -> Step {
    if !moved && ctx.now >= since + LID_UP {
        let next = if deposit {
            self::deposit(ctx, projects, job, body)
        } else {
            self::deposit(ctx, projects, job, body);
            let emptied = Body {
                slots: container_get(ContainerAddress::Mob(body.id)).unwrap_or_default(),
                ..*body
            };
            fetch(ctx, projects, job, &emptied, container)
        };
        job.crew.presence.jab(body.id);
        if next != Step::Plan {
            container_hold(ContainerAddress::Block(container), body.actor(), false);
            return next;
        }
        return Step::Rummage {
            container,
            deposit,
            since,
            moved: true,
        };
    }
    if moved && ctx.now >= since + LID_UP + LINGER {
        container_hold(ContainerAddress::Block(container), body.actor(), false);
        job.crew.presence.face(body.id, None);
        return Step::Plan;
    }
    Step::Rummage {
        container,
        deposit,
        since,
        moved,
    }
}

pub fn deposit(ctx: &mut Ctx, projects: &mut Projects, job: &mut Job, body: &Body) -> Step {
    let Some(project) = projects.get(job.id).cloned() else {
        return Step::Plan;
    };
    let everything = hands_in_everything(&project);
    let containers: Vec<[i32; 3]> = ctx
        .supplies
        .chain(project.table)
        .into_iter()
        .filter(|c| at_container(body, *c))
        .collect();
    let mut back = Vec::new();
    let mut kept = Vec::new();
    for (slot, stack) in body.slots.iter().enumerate() {
        let Some(stack) = stack else { continue };
        if stack.item == BLUEPRINT {
            continue;
        }
        if everything || !keeps(ctx, job, &body.slots, stack) {
            back.push(slot);
        } else if tool_kind(ctx, &stack.item).is_none()
            && !scaffold::keeps(ctx, job, &body.slots, stack)
        {
            kept.push((slot, needed_at(job, stack)));
        }
    }
    let free = body.slots.iter().filter(|s| s.is_none()).count() + back.len();
    let junk = back.len();
    let dig_room = dig_room(job);
    if !everything && free < dig_room && digs_ahead(job) {
        kept.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        back.extend(kept.iter().take(dig_room - free).map(|(slot, _)| *slot));
    }
    let mut stuck = false;
    for (n, slot) in back.into_iter().enumerate() {
        let Some(stack) = &body.slots[slot] else {
            continue;
        };
        let mut left = stack.count;
        for container in &containers {
            if left == 0 {
                break;
            }
            if let Some(moved) = container_transfer(
                ContainerAddress::Mob(body.id),
                slot as u32,
                ContainerAddress::Block(*container),
                left,
            ) {
                left -= moved.count;
            }
        }
        stuck |= left > 0 && n < junk;
    }
    if stuck && project.phase() == Phase::Returning {
        job.crew.cargo.chests_full = true;
        projects.update(job.id, |p| {
            p.note = std::mem::take(&mut p.note).with_chests_full()
        });
    } else if stuck {
        projects.update(job.id, |p| p.hold_for(Hold::Storage, Note::ChestsFull));
    }
    Step::Plan
}
