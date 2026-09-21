//! The Mason Golem at work.
//!
//! The tick system owns the golem's work: it reads the golem's body and
//! carried slots, decides the next step, and acts through the generic actor
//! calls (dig, place, transfer). The golem's brain node only steers — it
//! follows the goal and facing this module leaves in the golem's tags — so
//! the job survives a reload with nothing but the tags and the project
//! record: every other piece of state here is re-derived from the world.

/// Log a diagnostic line. Compiled out unless [`TRACE`] is set.
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::worker::TRACE {
            mod_sdk::log(&format!($($arg)*));
        }
    };
}

mod act;
mod aloft;
mod body;
mod bridge;
mod cargo;
mod course;
mod crew;
mod drive;
mod ground;
mod hands;
mod legs;
mod lifecycle;
mod pillar;
mod plan;
mod pocket;
mod presence;
mod rescue;
mod route;
mod scaffold;
mod sight;
mod stance;
mod step;
mod support;
pub mod trouble;
mod tuning;
mod upkeep;
mod waiting;
mod walk;
mod wayin;

use mod_sdk::*;

use crate::caches::Caches;
use crate::content::Content;
use crate::geometry::cell_of;
use crate::jobs::Job;
use crate::project::{Phase, Projects};
use crate::supplies::Supplies;

pub use act::acted;
pub use body::Body;
pub use crew::Crew;
pub use lifecycle::summon;
pub use pillar::Pillar;
pub use route::{Regions, Routes};
pub use step::{Step, Task, Then};

use body::{standing_cell, unwedge};
use drive::{act_out, arrive};
use legs::{centre, hold_still, lean};
use presence::eye_of;
use tuning::every::MEND_EVERY;
use tuning::reach::{SITE_ABOVE, SITE_AROUND};
use upkeep::{hold_for_blueprint, mend, open_block};
use walk::walk;

pub const PROJECT_TAG: &str = "builder:project";
pub const GOAL_TAG: &str = "builder:goal";
pub const HOLD_TAG: &str = "builder:hold";
pub const FACE_TAG: &str = "builder:face";
/// The cell the golem's head turns to while it works on it.
pub const LOOK_TAG: &str = "builder:look";
/// The golem row's `eye_height`.
pub const EYE_HEIGHT: f64 = 1.3;
/// The golem's health when summoned, which it mends back toward.
pub const FULL_HEALTH_TAG: &str = "builder:full_health";
const HEALTH_TAG: &str = "petramond:health";

pub const TRACE: bool = false;

pub struct Ctx<'a> {
    pub now: u64,
    pub content: &'a Content,
    pub supplies: &'a Supplies,
    pub caches: &'a mut Caches,
    /// Route-search nodes this tick may still spend.
    pub probe_nodes: &'a mut u32,
    /// Recent route answers by `(from, to, blocked cells' hash)` and the tick
    /// they were asked.
    pub routes: &'a mut route::Routes,
    /// Recent walkable-region floods over the site.
    pub regions: &'a mut route::Regions,
    /// Golems a player has a panel open on, and who.
    pub asked: &'a [(u64, PlayerId)],
    /// The box the job's walking is judged in: the design with room around it
    /// and home.
    pub site: ([i32; 3], [i32; 3]),
}

/// The box a job's walking is judged in: the design, the ground around it a
/// golem works from, and home.
pub fn site(job: &Job, home: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    let (min, max) = (job.design.min, job.design.max);
    (
        std::array::from_fn(|i| (min[i] - SITE_AROUND[i]).min(home[i] - 2)),
        std::array::from_fn(|i| {
            let extra = if i == 1 { SITE_ABOVE } else { SITE_AROUND[i] };
            (max[i] + extra).max(home[i] + 2)
        }),
    )
}

pub fn tick(ctx: &mut Ctx, projects: &mut Projects, job: &mut Job) {
    let Some(project) = projects.get(job.id).cloned() else {
        return;
    };
    if !project.phase().active() || !project.worker() {
        return;
    }
    let Some(id) = job.crew.find(ctx.now, project.id) else {
        return;
    };
    let Some(info) = mob_info(id) else {
        return;
    };
    if info.kind != ctx.content.golem {
        return;
    }
    if ctx.now.is_multiple_of(MEND_EVERY) {
        mend(id, info.health);
    }
    let body = Body {
        id,
        pos: info.pos,
        cell: if info.on_ground {
            standing_cell(info.pos)
        } else {
            cell_of(info.pos)
        },
        on_ground: info.on_ground,
        yaw: info.yaw,
        slots: container_get(ContainerAddress::Mob(id)).unwrap_or_default(),
    };
    if TRACE && ctx.now.is_multiple_of(2000) {
        let held: Vec<String> = body
            .slots
            .iter()
            .flatten()
            .filter(|s| scaffold::keeps(ctx, job, &body.slots, s) || s.item.contains("dirt"))
            .map(|s| format!("{}x{}", s.count, s.item))
            .collect();
        log(&format!(
            "TRACE scaffolding t={} in hand {held:?} standing {} free slots {}",
            ctx.now,
            project.scaffolds.len(),
            body.slots.iter().filter(|s| s.is_none()).count()
        ));
    }
    if project.phase() == Phase::Working {
        hold_for_blueprint(projects, &project, &body);
    }
    job.crew.scaffolding.block = scaffold::record(ctx, job, &body);
    job.crew.scaffolding.cells = project.scaffolds.iter().copied().collect();
    if ctx.asked.iter().any(|(mob, _)| *mob == id) {
        if job.crew.presence.asked_about.is_none() {
            job.crew.presence.asked_about = Some(trouble::of(&project, &job.crew, ctx.now));
        }
    } else {
        job.crew.presence.asked_about = None;
    }
    act_out(ctx, projects, job, &project, &body);
    if !matches!(job.crew.step, Step::Plan) {
        job.crew.pace.busy_at = ctx.now;
    }
    let trouble = trouble::of(&project, &job.crew, ctx.now);
    trouble::show(ctx, &mut job.crew, &body, trouble);
}
