//! Per-project session state and the tick that drives it: compiling the
//! anchored design, surveying it while anyone looks at it or a golem works
//! it, and handing each working golem its turn.
//!
//! - [`admission`] — whether Start may go ahead, and why not.
//! - [`holds`] — pausing, resuming, cancelling, and the holds that lift
//!   themselves.
//! - [`ghosts`] — the ghosts standing in the world.

mod admission;
mod ghosts;
mod holds;

use std::collections::BTreeMap;

use mod_sdk::*;

use crate::caches::Caches;
use crate::content::Content;
use crate::design::{Design, Progress};
use crate::fx::HashMap;
use crate::project::{Brief, ProjectId, Projects};
use crate::supplies::Supplies;
use crate::survey::{Summary, Survey};
use crate::worker::{self, Crew};

pub use admission::Refusal;

/// Stored sections compiled per tick, across every project.
const COMPILE_SECTIONS: u32 = 8;
/// Units measured per tick while someone watches a table, and while a golem
/// works unwatched.
const SURVEY_WATCHED: usize = 1024;
const SURVEY_WORKING: usize = 256;
/// A project nobody has looked at or worked for this long is dropped from
/// memory (its record stays in the world), and so is everything else the
/// session remembers on its behalf.
const FORGET_AFTER: u64 = 1200;
const FORGET: Cadence = Cadence::every(FORGET_AFTER);
/// Route-search nodes the golems may spend per tick.
const PROBE_NODES: u32 = 28_000;
/// How long a route answer stands, and how often the stale ones are dropped.
const ROUTE_TICKS: u64 = 200;
const ROUTE_SWEEP: Cadence = Cadence::every(ROUTE_TICKS);
/// How often a live project checks its table still stands and, working, that
/// its supplies still cover the rest.
const TABLE_CHECK: Cadence = Cadence::every(40);

pub struct Job {
    pub id: ProjectId,
    pub design: Design,
    pub survey: Option<Survey>,
    pub failed: Option<String>,
    pub crew: Crew,
    seen: u64,
}

impl Job {
    pub fn summary(&self) -> Option<&Summary> {
        self.survey.as_ref()?.summary()
    }

    /// The fraction of the design's units the world holds.
    pub fn done(&self) -> f32 {
        let Some(survey) = self.survey.as_ref() else {
            return 0.0;
        };
        if survey.known.is_empty() {
            return 1.0;
        }
        1.0 - survey.open() as f32 / survey.known.len() as f32
    }
}

#[derive(Default)]
pub struct Jobs {
    pub map: BTreeMap<ProjectId, Job>,
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
        let job = self.map.entry(project.id).or_insert_with(|| Job {
            id: project.id,
            design: Design::new(asset, origin, turns),
            survey: None,
            failed: None,
            crew: Crew::default(),
            seen: now,
        });
        job.seen = now;
        Some(job)
    }

    pub fn by_mob(&mut self, mob: u64) -> Option<&mut Job> {
        self.map
            .values_mut()
            .find(|j| j.crew.mob == Some(mob) || j.crew.last_mob == Some(mob))
    }
}

pub struct Builder {
    pub content: Content,
    pub supplies: Supplies,
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
        let live: Vec<ProjectId> = self.projects.live().collect();
        self.cancel_tableless(&live, now);
        let viewers = gui_viewers();
        let watched = crate::table::watched_projects(self, &viewers);
        let asked = crate::golem::asked(&viewers);
        let mut attended: Vec<(ProjectId, bool)> = watched.iter().map(|id| (*id, true)).collect();
        attended.extend(
            live.iter()
                .filter(|id| !watched.contains(id))
                .map(|id| (*id, false)),
        );
        // The tick's budgets are one for every golem: whoever goes first may
        // spend them, so the first turn goes round.
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
        self.jobs
            .map
            .retain(|_, j| now < j.seen + FORGET_AFTER || j.crew.mob.is_some());
        self.ghosts
            .sync(&self.content, &mut self.projects, &live, now);
        if ROUTE_SWEEP.due(now, 0) {
            self.routes.retain(|_, (_, at)| now < *at + ROUTE_TICKS);
        }
        if FORGET.due(now, 0) {
            self.projects.sweep();
            self.supplies.sweep(now);
            self.tables.sweep(now, FORGET_AFTER);
        }
    }

    /// Compile more of the job's design out of what is left of the tick's
    /// sections.
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

    /// The job and everything the worker may touch while it has it. `home`
    /// is the project's, where the caller already knows it.
    fn at_work<'a>(
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

    /// Fill every open panel. Runs right AFTER the menu stage, where a panel
    /// opens: an unset `enabled`/`visible` reads as true, so a panel filled a
    /// tick later flashes every button live first. A panel seen for the first
    /// time is filled at once; the rest keep their cadence.
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
