mod act;
mod backoff;
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
use crate::project::{ProjectId, Projects};
use crate::testing::{Session, HOME};
use crate::worker::Job;

const NODES: u32 = 28_000;

pub(super) fn working(at: [i32; 3]) -> (Session, ProjectId, u64) {
    let (mut session, id) = Session::row();
    session.job(id);
    let golem = session.golem(id, HOME, at);
    (session, id, golem)
}

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

pub(super) fn unit(session: &Session, id: ProjectId, pos: [i32; 3]) -> usize {
    session.builder.jobs.map[&id]
        .design
        .unit_at(pos)
        .expect("a unit there")
}
