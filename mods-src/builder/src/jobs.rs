mod admission;
mod ghosts;
mod holds;

use std::collections::BTreeMap;

use crate::host::prelude::*;

use crate::caches::Caches;
use crate::content::Content;
use crate::design::{Design, Progress};
use crate::fx::HashMap;
use crate::project::{Brief, ProjectId, Projects};
use crate::supplies::Supplies;
use crate::survey::Survey;
use crate::worker::{self, Job};

pub use admission::Refusal;

const COMPILE_SECTIONS: u32 = 8;
const SURVEY_WATCHED: usize = 1024;
const SURVEY_WORKING: usize = 256;
const FORGET_AFTER: u64 = 1200;
const FORGET: Cadence = Cadence::every(FORGET_AFTER);
const PROBE_NODES: u32 = 28_000;
const ROUTE_TICKS: u64 = 200;
const ROUTE_SWEEP: Cadence = Cadence::every(ROUTE_TICKS);
const TABLE_CHECK: Cadence = Cadence::every(40);

#[derive(Default)]
pub struct Jobs {
    pub map: BTreeMap<ProjectId, Job>,
    mobs: HashMap<u64, ProjectId>,
    seen: BTreeMap<ProjectId, u64>,
}

impl Jobs {
    pub fn attend(&mut self, project: &Brief, now: u64) -> Option<&mut Job> {
        let (asset, origin, turns) = project.anchored?;
        let stale = self
            .map
            .get(&project.id)
            .is_some_and(|j| !j.design.matches(asset, origin, turns));
        if stale {
            self.map.remove(&project.id);
        }
        self.seen.insert(project.id, now);
        let job = self
            .map
            .entry(project.id)
            .or_insert_with(|| Job::new(project.id, Design::new(asset, origin, turns)));
        Some(job)
    }

    pub fn by_mob(&mut self, mob: u64) -> Option<&mut Job> {
        let works = |j: &Job| j.crew.mob == Some(mob) || j.crew.last_mob == Some(mob);
        let id = match self.mobs.get(&mob) {
            Some(id) if self.map.get(id).is_some_and(works) => *id,
            _ => {
                let id = self.map.values().find(|j| works(j))?.id;
                self.mobs.insert(mob, id);
                id
            }
        };
        self.map.get_mut(&id)
    }

    fn forget_idle(&mut self, now: u64) {
        let seen = &self.seen;
        self.map.retain(|id, j| {
            seen.get(id).is_some_and(|at| now < at + FORGET_AFTER) || j.crew.mob.is_some()
        });
        let map = &self.map;
        self.mobs.retain(|_, id| map.contains_key(id));
        self.seen.retain(|id, _| map.contains_key(id));
    }
}

pub struct Builder {
    pub content: Content,
    supplies: Supplies,
    pub projects: Projects,
    pub jobs: Jobs,
    pub caches: Caches,
    pub panels: PanelPublisher,
    pub tables: crate::table::Tables,
    routes: worker::Routes,
    regions: worker::Regions,
    changes: ChangeCursor,
    ghosts: ghosts::Ghosts,
    admissions: admission::Admissions,
}

impl Builder {
    pub fn new(content: Content) -> Self {
        Self {
            content,
            supplies: Supplies::resolve(),
            projects: Projects::load(),
            jobs: Jobs::default(),
            caches: Caches::default(),
            panels: PanelPublisher::default(),
            tables: crate::table::Tables::default(),
            routes: HashMap::default(),
            regions: HashMap::default(),
            changes: ChangeCursor::default(),
            ghosts: ghosts::Ghosts::default(),
            admissions: admission::Admissions::default(),
        }
    }

    pub fn tick(&mut self) {
        let now = current_tick();
        let changes = self.changes.advance();
        self.supplies.changed(&changes.cells, changes.lost);
        let table_due = self.projects.table_check_due(now).to_vec();
        self.cancel_tableless(&table_due, now);
        let refresh_ghosts = self.ghosts.due(now);
        let live: Vec<ProjectId> = if refresh_ghosts {
            self.projects.live().collect()
        } else {
            Vec::new()
        };
        let viewers = gui_viewers();
        let watched = crate::table::watched_projects(self, &viewers);
        let asked = crate::golem::asked(&viewers);
        let mut attended: Vec<(ProjectId, bool)> = watched.iter().map(|id| (*id, true)).collect();
        attended.extend(
            self.projects
                .active()
                .filter(|id| !watched.contains(id))
                .map(|id| (id, false)),
        );
        if !attended.is_empty() {
            let first = (now % attended.len() as u64) as usize;
            attended.rotate_left(first);
        }
        let mut compile_sections = COMPILE_SECTIONS;
        let mut probe_nodes = PROBE_NODES;
        for (id, watched) in attended {
            let Some(project) = self.projects.get(id).map(|p| p.brief()) else {
                continue;
            };
            let active = project.phase.active();
            if !watched && !active {
                continue;
            }
            if self.jobs.attend(&project, now).is_none() {
                continue;
            }
            self.compile(id, &mut compile_sections);
            if active && TABLE_CHECK.due(now, id) {
                self.check_supplies(&project, now);
            }
            let Some(job) = self.jobs.map.get_mut(&id) else {
                continue;
            };
            if changes.lost || changes.cells.iter().any(|c| job.design.near(*c)) {
                job.crew.site_changed();
            }
            if let Some(survey) = job.survey.as_mut() {
                let budget = if watched {
                    SURVEY_WATCHED
                } else {
                    SURVEY_WORKING
                };
                survey.step(&job.design, budget, now, id, &changes.cells, changes.lost);
            }
            if active {
                if let Some((mut ctx, projects, job)) =
                    self.at_work(id, Some(project.home), now, &mut probe_nodes, &asked)
                {
                    worker::tick(&mut ctx, projects, job);
                }
            }
        }
        self.jobs.forget_idle(now);
        if refresh_ghosts {
            self.ghosts.sync(&self.content, &mut self.projects, &live);
        }
        if ROUTE_SWEEP.due(now, 0) {
            self.routes.retain(|_, (_, at)| now < *at + ROUTE_TICKS);
        }
        if FORGET.due(now, 0) {
            self.projects.sweep();
            self.supplies.sweep(now);
            self.tables.sweep(now, FORGET_AFTER);
        }
    }

    fn compile(&mut self, id: ProjectId, sections: &mut u32) {
        let Some(job) = self.jobs.map.get_mut(&id) else {
            return;
        };
        if job.design.complete || job.failed.is_some() || *sections == 0 {
            return;
        }
        let before = job.design.compiled();
        let progress = job.design.compile(*sections);
        *sections = sections.saturating_sub(job.design.compiled() - before);
        match progress {
            Progress::Compiling => {}
            Progress::Failed(reason) => job.failed = Some(reason),
            Progress::Ready => {
                job.survey = Some(Survey::new(&job.design));
                let title = job.design.title.clone();
                if self.projects.get(id).is_some_and(|p| p.title != title) {
                    self.projects.update(id, |p| p.title = title);
                    crate::table::label_blueprint(self, id);
                }
            }
        }
    }

    pub(crate) fn at_work<'a>(
        &'a mut self,
        id: ProjectId,
        home: Option<[i32; 3]>,
        now: u64,
        probe_nodes: &'a mut u32,
        asked: &'a [(u64, PlayerId)],
    ) -> Option<(worker::Ctx<'a>, &'a mut Projects, &'a mut Job)> {
        let job = self.jobs.map.get_mut(&id)?;
        let home = home
            .or_else(|| self.projects.get(id).map(|p| p.home))
            .unwrap_or(job.design.min);
        let ctx = worker::Ctx {
            now,
            content: &self.content,
            supplies: &self.supplies,
            caches: &mut self.caches,
            probe_nodes,
            routes: &mut self.routes,
            regions: &mut self.regions,
            asked,
            site: worker::site(job, home),
        };
        Some((ctx, &mut self.projects, job))
    }

    pub fn publish_panels(&mut self) {
        let now = current_tick();
        let viewers = gui_viewers();
        let closed = self.panels.observe(&viewers);
        crate::table::closed(self, &closed, now);
        crate::table::follow_blueprints(self, now);
        crate::table::publish(self, now, &viewers);
        crate::golem::publish(self, now, &viewers);
    }

    pub fn acted(
        &mut self,
        mob: u64,
        pos: [i32; 3],
        action: ActorAction,
        refusal: Option<ActionRefusal>,
    ) {
        let Some(id) = self.jobs.by_mob(mob).map(|job| job.id) else {
            return;
        };
        let mut probe_nodes = 0;
        if let Some((mut ctx, projects, job)) =
            self.at_work(id, None, current_tick(), &mut probe_nodes, &[])
        {
            worker::acted(&mut ctx, projects, job, pos, action, refusal);
        }
    }
}

#[cfg(test)]
mod tests;
