use crate::host::prelude::*;

use super::Pillar;
use crate::geometry::offset;
use crate::project::{Project, Projects};
use crate::worker::legs::{self, centre_on, hold_still};
use crate::worker::tuning::body::{JUMP, ON_COLUMN, PERCH_OFF_CENTRE, PERCH_SETTLE_TICKS};
use crate::worker::tuning::hands::SWING;
use crate::worker::tuning::patience::{CLIMB_STALL, DESCENT_STALL};
use crate::worker::Job;
use crate::worker::{centre, hands, open_block, scaffold, Body, Ctx, Step};

#[allow(clippy::too_many_arguments)]
pub fn climb(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    pillar: Pillar,
    level: Option<i32>,
    placed: bool,
    since: u64,
) -> Step {
    let id = body.id;
    job.crew.presence.set_goal(id, None);
    job.crew.presence.set_hold(id, true);
    job.crew.presence.animate(id, None);
    job.crew.aloft.descending = None;
    let [cx, cz] = pillar.column;
    job.crew.presence.look_at(
        id,
        ctx.now,
        body.pos,
        [f64::from(cx) + 0.5, body.pos[1] - 1.0, f64::from(cz) + 0.5],
    );
    let on_column = pillar.on_column(body.cell);
    if ctx.now > since + CLIMB_STALL || !on_column {
        if let Some((task, _)) = job.crew.aloft.climbed_for {
            job.crew.deferrals.strike(task, [cx, pillar.top, cz]);
        }
        job.crew.aloft.perch = Some(Pillar {
            top: body.cell[1],
            ..pillar
        });
        return Step::Descend { since: ctx.now };
    }
    if body.on_ground {
        if body.cell[1] >= pillar.top {
            job.crew.aloft.perch = Some(Pillar {
                top: body.cell[1],
                ..pillar
            });
            job.crew.aloft.settle_until = ctx.now + PERCH_SETTLE_TICKS;
            centre(body);
            return Step::Plan;
        }
        let [dx, dz] = body.off_centre();
        if dx.abs() > PERCH_OFF_CENTRE || dz.abs() > PERCH_OFF_CENTRE {
            centre(body);
            return Step::Climb {
                pillar,
                level,
                placed,
                since,
            };
        }
        if ctx.now < job.crew.presence.gaze_since + SWING {
            return Step::Climb {
                pillar,
                level,
                placed,
                since,
            };
        }
        legs::jump(body, JUMP);
        return Step::Climb {
            pillar,
            level: Some(body.cell[1]),
            placed: false,
            since,
        };
    }
    hold_still(body);
    if let Some(l) = level {
        if l < pillar.top && body.pos[1] >= f64::from(l) + 1.02 {
            let cell = [cx, l, cz];
            match scaffold::lay(ctx, job, body, cell) {
                hands::Lay::Queued | hands::Lay::Satisfied => {
                    projects.update(job.id, |p| {
                        if !p.scaffolds.contains(&cell) {
                            p.scaffolds.push(cell);
                        }
                    });
                    return Step::Climb {
                        pillar,
                        level: Some(l + 1),
                        placed: true,
                        since: ctx.now,
                    };
                }
                hands::Lay::Turning | hands::Lay::Refused(ActionRefusal::BodyInTheWay) => {}
                hands::Lay::Refused(ActionRefusal::MissingItems) => {
                    job.crew.scaffolding.want = (pillar.top - pillar.base).max(0) as u32;
                    job.crew.scaffolding.short = true;
                    job.crew.aloft.perch = Some(Pillar { top: l, ..pillar });
                    return Step::Descend { since: ctx.now };
                }
                hands::Lay::Refused(_) => {
                    job.crew.note = "Scaffolding could not be placed".into();
                    if let Some((task, _)) = job.crew.aloft.climbed_for {
                        job.crew.deferrals.strike(task, [cx, pillar.top, cz]);
                    }
                    job.crew.aloft.perch = Some(Pillar { top: l, ..pillar });
                    return Step::Descend { since: ctx.now };
                }
            }
        }
    }
    Step::Climb {
        pillar,
        level,
        placed,
        since,
    }
}

pub fn descend(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    since: u64,
) -> Step {
    let id = body.id;
    job.crew.presence.set_goal(id, None);
    job.crew.presence.set_hold(id, true);
    let Some(perch) = job.crew.aloft.perch else {
        job.crew.presence.set_hold(id, false);
        return Step::Plan;
    };
    let at = [perch.column[0], body.cell[1], perch.column[1]];
    let below = offset(at, [0, -1, 0]);
    let ours = projects
        .get(job.id)
        .is_some_and(|p| p.scaffolds.contains(&below))
        && scaffold::stands(ctx.content, below) == Some(true);
    if !body.on_ground {
        return Step::Descend { since };
    }
    let on_column = (body.pos[0] - (f64::from(perch.column[0]) + 0.5)).abs() < ON_COLUMN
        && (body.pos[2] - (f64::from(perch.column[1]) + 0.5)).abs() < ON_COLUMN;
    if !ours || !on_column || ctx.now > since + DESCENT_STALL {
        hold_still(body);
        job.crew.aloft.dismount_down();
        job.crew.presence.set_hold(id, false);
        return Step::Plan;
    }
    let floor = offset(below, [0, -1, 0]);
    if get_block(floor).is_none_or(|b| open_block(ctx, b)) {
        hold_still(body);
        job.crew.note = "The golem's scaffolding has a gap below it".into();
        job.crew.aloft.dismount_down();
        job.crew.presence.set_hold(id, false);
        return Step::Plan;
    }
    let off = (body.pos[0] - (f64::from(perch.column[0]) + 0.5))
        .abs()
        .max((body.pos[2] - (f64::from(perch.column[1]) + 0.5)).abs());
    if off > PERCH_OFF_CENTRE {
        centre_on(body, at);
        return Step::Descend { since };
    }
    if job
        .crew
        .aloft
        .descending
        .is_some_and(|level| level != body.cell[1])
    {
        job.crew.aloft.descending = Some(body.cell[1]);
        job.crew.aloft.perch = Some(Pillar {
            top: body.cell[1],
            ..perch
        });
        return Step::Plan;
    }
    hold_still(body);
    match hands::dig(ctx, job, body, below, true) {
        hands::Dig::Breaking | hands::Dig::Nothing => {
            projects.update(job.id, |p| p.scaffolds.retain(|c| *c != below));
            Step::Descend { since: ctx.now }
        }
        hands::Dig::Turning | hands::Dig::Digging => Step::Descend { since },
        hands::Dig::Refused(_) => {
            job.crew.note = "The golem is stuck on its scaffolding".into();
            Step::Descend { since }
        }
    }
}

pub fn recover(ctx: &mut Ctx, project: &Project, body: &Body) -> Option<Pillar> {
    let below = offset(body.cell, [0, -1, 0]);
    if !project.scaffolds.contains(&below) || scaffold::stands(ctx.content, below) != Some(true) {
        return None;
    }
    let mut base = below[1];
    while project.scaffolds.contains(&[below[0], base - 1, below[2]]) {
        base -= 1;
    }
    let ground = [below[0], base - 1, below[2]];
    if get_block(ground).is_none_or(|b| open_block(ctx, b)) {
        return None;
    }
    Some(Pillar {
        column: [below[0], below[2]],
        base,
        top: body.cell[1],
        exit: body.cell,
        onward: None,
    })
}
