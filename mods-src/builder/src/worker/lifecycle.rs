//! Summoning, emerging and burrowing: the golem rises out of the ground
//! beside the table and sinks back into it, the body authored along the way
//! so it passes through the terrain without changing it.

use mod_sdk::*;

use super::presence::Presentation;
use super::tuning::body::{BURROW_DEPTH, BURROW_TICKS, EMERGE_TICKS, TRAVEL_DEPTH, TRAVEL_STEP};
use super::{open_block, Body, Crew, Ctx, Step, FULL_HEALTH_TAG, HOLD_TAG, PROJECT_TAG};
use crate::content::{BLUEPRINT, EARTH_BURST, GOLEM};
use crate::geometry::{feet_of, offset};
use crate::jobs::Job;
use crate::project::{Project, Projects};
pub const EMERGE: &str = "emerge";
pub const BURROW: &str = "burrow";

/// Open ground near the table the golem can rise out of.
pub fn find_home(ctx: &mut Ctx, job: &Job, table: [i32; 3]) -> Option<[i32; 3]> {
    let mut candidates = Vec::new();
    for r in 1..=4i32 {
        for dy in [0, -1, 1, -2] {
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let cell = offset(table, [dx, dy, dz]);
                    let head = offset(cell, [0, 1, 0]);
                    let governed = &job.design.governed;
                    if !governed.contains(&cell) && !governed.contains(&head) {
                        candidates.push(cell);
                    }
                }
            }
        }
    }
    let fits = homes(ctx, table, &candidates);
    candidates
        .into_iter()
        .zip(fits)
        .find(|(_, fits)| *fits)
        .map(|(cell, _)| cell)
}

/// Which of `cells` the golem can rise out of and sink back into: open, room
/// to stand, and a whole block under it that is neither the table nor a
/// supply chest.
pub fn homes(ctx: &mut Ctx, table: [i32; 3], cells: &[[i32; 3]]) -> Vec<bool> {
    let standing = footholds(GOLEM, cells.to_vec());
    let here = get_blocks(cells.to_vec());
    let grounds = get_blocks(cells.iter().map(|c| offset(*c, [0, -1, 0])).collect());
    let supply = ctx.supplies.chain(table);
    let mut fits = Vec::with_capacity(cells.len());
    for (((cell, ok), here), ground) in cells.iter().zip(standing).zip(here).zip(grounds) {
        let below = offset(*cell, [0, -1, 0]);
        fits.push(
            ok && here.is_some_and(|b| open_block(ctx, b))
                && ground.is_some_and(|b| {
                    b != ctx.content.table
                        && !supply.contains(&below)
                        && ctx.caches.block(b).is_some_and(|info| {
                            matches!(info.collision.as_slice(), [(lo, hi)] if *lo == [0.0; 3] && *hi == [1.0; 3])
                        })
                }),
        );
    }
    fits
}

pub fn summon(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    project: &Project,
) -> Result<(), String> {
    let home = find_home(ctx, job, project.table)
        .ok_or("There is no open ground beside the table for the golem")?;
    let feet = feet_of(home);
    let pos = [feet[0], feet[1] - BURROW_DEPTH, feet[2]];
    let yaw = crate::geometry::yaw_toward(feet, project.table);
    let id = spawn_mob(GOLEM, pos, yaw).ok_or("The golem could not be summoned")?;
    mob_tag_set(id, PROJECT_TAG, MobTagValue::I64(project.id as i64));
    mob_tag_set(id, HOLD_TAG, MobTagValue::Bool(true));
    if let Some(info) = mob_info(id) {
        mob_tag_set(
            id,
            FULL_HEALTH_TAG,
            MobTagValue::F64(f64::from(info.health)),
        );
    }
    mob_kinematic(id, pos, yaw, 0.0, 0.0);
    projects.update(project.id, |p| p.summon(home));
    job.crew = Crew {
        mob: Some(id),
        last_mob: Some(id),
        step: Step::Emerge { t: 0 },
        presence: Presentation {
            hold: true,
            ..Presentation::default()
        },
        ..Crew::default()
    };
    Ok(())
}

fn earth(home: [i32; 3], t: u32) {
    let feet = feet_of(home);
    if t.is_multiple_of(6) {
        // Thrown up out of whatever it comes through: the ground's own
        // flecks, as digging it would shed them. The bundle's brown is for
        // ground that cannot be read.
        let ground = get_block([home[0], home[1] - 1, home[2]]);
        emitter_burst_of(
            EARTH_BURST,
            [feet[0], feet[1] + 0.05, feet[2]],
            1.5,
            None,
            ground.map(|block| ParticleTexture::Block { block, tint: None }),
        );
    }
    if t.is_multiple_of(12) {
        sound_play_at("petramond:dirt_break", feet, 0.9, 0.7);
    }
}

pub fn emerge(projects: &mut Projects, job: &mut Job, body: &Body) {
    let Some(project) = projects.get(job.id).cloned() else {
        return;
    };
    let t = match job.crew.step {
        Step::Emerge { t } => t,
        _ => 0,
    };
    let yaw = crate::geometry::yaw_toward(feet_of(project.home), project.table);
    job.crew.presence.set_hold(body.id, true);
    job.crew.presence.animate(body.id, Some(EMERGE));
    if t >= EMERGE_TICKS {
        job.crew.presence.animate(body.id, None);
        job.crew.presence.set_hold(body.id, false);
        // It comes up beside the table and takes its plans with it: from here
        // on the blueprint it carries is what it builds by.
        container_transfer(
            ContainerAddress::Block(project.table),
            0,
            ContainerAddress::Mob(body.id),
            1,
        );
        projects.update(job.id, Project::emerged);
        job.crew.pace.progress_at = current_tick();
        job.crew.pace.band_progress_at = current_tick();
        job.crew.step = Step::Plan;
        return;
    }
    let feet = feet_of(project.home);
    let rise = f64::from(t) / f64::from(EMERGE_TICKS);
    mob_kinematic(
        body.id,
        [feet[0], feet[1] - BURROW_DEPTH * (1.0 - rise), feet[2]],
        yaw,
        0.0,
        0.0,
    );
    earth(project.home, t);
    job.crew.step = Step::Emerge { t: t + 1 };
}

/// Stuck past getting out: sink into the ground where the golem stands, pass
/// home under the ground, and rise again there, carrying what it carried.
/// What it was up on or laying is left for the planner to take down.
pub fn relocate(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    t: u32,
    leg: u8,
) -> Step {
    let Some(project) = projects.get(job.id).cloned() else {
        return Step::Plan;
    };
    if leg == 0 && t == 0 {
        job.crew.rescue.burrow_from = body.cell;
        job.crew.aloft.dismount_lost();
        if let Some(walkway) = job.crew.aloft.bridge.take() {
            for support in walkway.path.iter().map(|p| offset(*p, [0, -1, 0])) {
                if project.scaffolds.contains(&support)
                    && !job.crew.scaffolding.urgent.contains(&support)
                {
                    job.crew.scaffolding.urgent.push(support);
                }
            }
        }
    }
    job.crew.presence.still(body.id);
    job.crew.presence.set_hold(body.id, true);
    let start = feet_of(job.crew.rescue.burrow_from);
    let home = feet_of(project.home);
    let yaw = crate::geometry::yaw_toward(home, project.table);
    match leg {
        0 if t < BURROW_TICKS => {
            job.crew.presence.animate(body.id, Some(BURROW));
            let sink = f64::from(t) / f64::from(BURROW_TICKS);
            mob_kinematic(
                body.id,
                [start[0], start[1] - BURROW_DEPTH * sink, start[2]],
                body.yaw,
                0.0,
                0.0,
            );
            earth(job.crew.rescue.burrow_from, t);
            Step::Relocate { t: t + 1, leg }
        }
        0..=3 => {
            job.crew.presence.animate(body.id, None);
            let deep = start[1].min(home[1]) - BURROW_DEPTH - TRAVEL_DEPTH;
            let waypoints = [
                [start[0], deep, start[2]],
                [home[0], deep, home[2]],
                [home[0], home[1] - BURROW_DEPTH, home[2]],
            ];
            let leg = leg.max(1);
            let to = waypoints[usize::from(leg - 1)];
            let delta: [f64; 3] = std::array::from_fn(|i| to[i] - body.pos[i]);
            let length = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
            if length <= TRAVEL_STEP {
                mob_kinematic(body.id, to, yaw, 0.0, 0.0);
                return Step::Relocate { t: 0, leg: leg + 1 };
            }
            let k = TRAVEL_STEP / length;
            let pos = std::array::from_fn(|i| body.pos[i] + delta[i] * k);
            mob_kinematic(body.id, pos, yaw, 0.0, 0.0);
            Step::Relocate { t, leg }
        }
        _ if t < EMERGE_TICKS => {
            job.crew.presence.animate(body.id, Some(EMERGE));
            let rise = f64::from(t) / f64::from(EMERGE_TICKS);
            mob_kinematic(
                body.id,
                [home[0], home[1] - BURROW_DEPTH * (1.0 - rise), home[2]],
                yaw,
                0.0,
                0.0,
            );
            earth(project.home, t);
            Step::Relocate { t: t + 1, leg }
        }
        _ => {
            job.crew.presence.animate(body.id, None);
            job.crew.presence.set_hold(body.id, false);
            job.crew.trail.broken();
            job.crew.deferrals.tried.clear();
            job.crew.pace.progress_at = ctx.now;
            job.crew.pace.band_progress_at = ctx.now;
            ctx.routes.clear();
            ctx.regions.clear();
            Step::Plan
        }
    }
}

pub fn burrow(projects: &mut Projects, job: &mut Job, body: &Body) {
    let t = match job.crew.step {
        Step::Burrow { t } => t,
        _ => 0,
    };
    // The step arrives as `Burrow { t: 0 }` from the walk home: the first
    // tick of sinking is where it starts from.
    if t == 0 {
        job.crew.rescue.burrow_from = body.cell;
        // A job called off is still owed: its plans go back into the table,
        // where Start takes the same build up again. (A finished job's go
        // down with the golem.)
        if let Some(project) = projects.get(job.id).filter(|p| p.cancelling()).cloned() {
            let bound = |stack: &ItemStackData| projects.bound(stack) == Some(project.id);
            if let Some(slot) = body
                .slots
                .iter()
                .position(|s| s.as_ref().is_some_and(bound))
            {
                let tabled = container_transfer(
                    ContainerAddress::Mob(body.id),
                    slot as u32,
                    ContainerAddress::Block(project.table),
                    1,
                );
                // No table to take them (it is gone, or its slot is taken):
                // the plans are left here on the ground. Carried down, they
                // scatter where the body leaves, at the bottom of its burrow.
                if let (None, Some(plans)) = (tabled, &body.slots[slot]) {
                    let data: Vec<(&str, &[u8])> = plans
                        .data
                        .iter()
                        .map(|(key, bytes)| (key.as_str(), bytes.as_slice()))
                        .collect();
                    let at = [body.pos[0], body.pos[1] + 0.5, body.pos[2]];
                    if spawn_item_data(&plans.item, 1, at, &data) {
                        container_set(ContainerAddress::Mob(body.id), vec![(slot as u32, None)]);
                    }
                }
            }
        }
    }
    let start = job.crew.rescue.burrow_from;
    job.crew.presence.still(body.id);
    job.crew.presence.set_hold(body.id, true);
    job.crew.presence.animate(body.id, Some(BURROW));
    if t >= BURROW_TICKS {
        // A body that leaves scatters what it carries; a finished job's plans
        // go down with it instead. A cancelled job's that found no room in
        // the table are left to scatter: the player is owed them.
        let cancelled = projects.get(job.id).is_some_and(|p| p.cancelling());
        let plans: Vec<(u32, Option<ItemStackData>)> = body
            .slots
            .iter()
            .enumerate()
            .filter(|_| !cancelled)
            .filter(|(_, stack)| stack.as_ref().is_some_and(|s| s.item == BLUEPRINT))
            .map(|(slot, _)| (slot as u32, None))
            .collect();
        if !plans.is_empty() {
            container_set(ContainerAddress::Mob(body.id), plans);
        }
        despawn_mob(body.id);
        projects.update(job.id, Project::gone_home);
        job.crew = Crew::default();
        return;
    }
    let feet = feet_of(start);
    let sink = f64::from(t) / f64::from(BURROW_TICKS);
    mob_kinematic(
        body.id,
        [feet[0], feet[1] - BURROW_DEPTH * sink, feet[2]],
        body.yaw,
        0.0,
        0.0,
    );
    earth(start, t);
    job.crew.step = Step::Burrow { t: t + 1 };
}
