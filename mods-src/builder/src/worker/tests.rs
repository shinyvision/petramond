//! The golem's work, a piece at a time, against the fake world.

mod act;
mod body;
mod build;
mod crew;
mod ground;
mod lifecycle;
mod route;
mod stance;
mod tick;
mod trouble;

use super::Ctx;
use crate::jobs::Job;
use crate::project::{ProjectId, Projects};
use crate::testing::{Session, HOME};

/// Route-search nodes a test tick may spend: the session's own budget.
const NODES: u32 = 28_000;

/// The row site with its job compiled and surveyed, and a golem out at work
/// from home, standing in `at`.
pub(super) fn working(at: [i32; 3]) -> (Session, ProjectId, u64) {
    let (mut session, id) = Session::row();
    session.job(id);
    let golem = session.golem(id, HOME, at);
    (session, id, golem)
}

/// Run `f` with what a tick lends the worker for project `id`.
pub(super) fn at_work<R>(
    session: &mut Session,
    id: ProjectId,
    f: impl FnOnce(&mut Ctx, &mut Projects, &mut Job) -> R,
) -> R {
    let now = session.now();
    let mut nodes = NODES;
    let (mut ctx, projects, job) = session
        .builder
        .at_work(id, None, now, &mut nodes, &[])
        .expect("the job is attended");
    f(&mut ctx, projects, job)
}

/// The design unit anchored at `pos`.
pub(super) fn unit(session: &Session, id: ProjectId, pos: [i32; 3]) -> usize {
    session.builder.jobs.map[&id]
        .design
        .unit_at(pos)
        .expect("a unit there")
}
