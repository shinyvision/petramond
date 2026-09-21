//! Whether Start (or Resume) may go ahead: the site is known, nothing guarded
//! is in the way, and the chests cover the bill. Asked by every panel
//! publish, so the answer is worked out once a tick per project.

use std::collections::BTreeMap;
use std::fmt;

use mod_sdk::*;

use crate::content::Content;
use crate::fx::{HashMap, HashSet};
use crate::jobs::{Builder, Jobs};
use crate::project::{Brief, Phase, ProjectId, Projects};
use crate::supplies::{self, Shortfall, Supplies};
use crate::survey::ItemKey;

/// Why a project cannot start. Rendered here and nowhere else.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    NoProject,
    AlreadyStarted,
    NotPositioned,
    BlueprintNotTabled,
    CheckingSite,
    /// The design did not compile.
    Design(String),
    NothingLeft,
    NotLoaded(u32),
    Guarded,
    CheckingSupplies,
    Short(Shortfall),
    GolemLostBlueprint,
    /// The golem could not be brought up.
    Summon(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoProject => f.write_str("No project"),
            Refusal::AlreadyStarted => f.write_str("Already started"),
            Refusal::NotPositioned => f.write_str("Position the ghost first"),
            Refusal::BlueprintNotTabled => f.write_str("Put its blueprint in the table"),
            Refusal::CheckingSite => f.write_str("Checking the site"),
            Refusal::Design(reason) | Refusal::Summon(reason) => f.write_str(reason),
            Refusal::NothingLeft => f.write_str("Nothing is left to build"),
            Refusal::NotLoaded(blocks) => write!(f, "{blocks} blocks are not loaded yet"),
            Refusal::Guarded => f.write_str("A container with items is in the way"),
            Refusal::CheckingSupplies => f.write_str("Checking the supplies"),
            Refusal::Short(short) => short.fmt(f),
            Refusal::GolemLostBlueprint => f.write_str("The golem is missing its blueprint"),
        }
    }
}

/// This tick's answers, by project and the table it would run from.
#[derive(Default)]
pub struct Admissions {
    tick: u64,
    answers: HashMap<(ProjectId, [i32; 3]), Result<(), Refusal>>,
}

impl Builder {
    /// Whether `project` may start, worked out at most once a tick.
    pub fn admission(&mut self, project: &Brief, now: u64) -> Result<(), Refusal> {
        if self.admissions.tick != now {
            self.admissions.tick = now;
            self.admissions.answers.clear();
        }
        let key = (project.id, project.table);
        if let Some(answer) = self.admissions.answers.get(&key) {
            return answer.clone();
        }
        let answer = self.admit(project, now);
        self.admissions.answers.insert(key, answer.clone());
        answer
    }

    fn admit(&mut self, project: &Brief, now: u64) -> Result<(), Refusal> {
        if project.anchored.is_none() {
            return Err(Refusal::NotPositioned);
        }
        // The golem takes its plans from the table when it comes up.
        if !project.worker
            && !blueprint_tabled(&self.content, &self.projects, project.id, project.table)
        {
            return Err(Refusal::BlueprintNotTabled);
        }
        // No job yet is no answer yet: Start would find nothing to start.
        let job = self
            .jobs
            .map
            .get(&project.id)
            .ok_or(Refusal::CheckingSite)?;
        if let Some(reason) = &job.failed {
            return Err(Refusal::Design(reason.clone()));
        }
        let summary = job.summary().ok_or(Refusal::CheckingSite)?;
        if project.resumable() && summary.open == 0 {
            return Err(Refusal::NothingLeft);
        }
        if summary.unchecked > 0 {
            return Err(Refusal::NotLoaded(summary.unchecked));
        }
        if summary.guarded > 0 {
            return Err(Refusal::Guarded);
        }
        let have = available(&self.supplies, &self.jobs, &self.projects, project, now);
        let short = supplies::shortfall(&summary.bill, &have);
        match supplies::worst(&short, &mut self.caches) {
            None => Ok(()),
            // A chest in a section that has not streamed in answers nothing:
            // what it holds is unknown, not missing.
            Some(_) if !self.supplies.stock_at(project.table, now).read => {
                Err(Refusal::CheckingSupplies)
            }
            Some(short) => Err(Refusal::Short(short)),
        }
    }

    /// What connected supplies offer `project`: see [`available`].
    pub fn available(&self, project: &Brief, now: u64) -> BTreeMap<ItemKey, u32> {
        available(&self.supplies, &self.jobs, &self.projects, project, now)
    }

    /// Start a golem on an anchored project whose supplies are complete.
    pub fn start(&mut self, id: ProjectId) -> Result<(), Refusal> {
        let project = self.projects.get(id).cloned().ok_or(Refusal::NoProject)?;
        let brief = project.brief();
        if brief.phase != Phase::Draft && !brief.lost_worker() && !brief.resumable() {
            return Err(Refusal::AlreadyStarted);
        }
        let now = current_tick();
        // A press is answered for the world as it is now, not as a panel
        // last saw it.
        self.admit(&brief, now)?;
        let mut probe_nodes = 0;
        let (mut ctx, projects, job) = self
            .at_work(id, Some(brief.home), now, &mut probe_nodes, &[])
            .ok_or(Refusal::CheckingSite)?;
        crate::worker::summon(&mut ctx, projects, job, &project).map_err(Refusal::Summon)
    }
}

/// What connected supplies offer `project`, less what other working jobs
/// sharing any of those chests still need.
fn available(
    supplies: &Supplies,
    jobs: &Jobs,
    projects: &Projects,
    project: &Brief,
    now: u64,
) -> BTreeMap<ItemKey, u32> {
    let stock = supplies.stock_at(project.table, now);
    let mine: HashSet<[i32; 3]> = stock.containers.iter().copied().collect();
    let mut totals = stock.totals.clone();
    if let Some(mob) = jobs.map.get(&project.id).and_then(|j| j.crew.mob) {
        if let Some(slots) = container_get(ContainerAddress::Mob(mob)) {
            supplies::add_totals(&mut totals, &slots);
        }
    }
    for (other, job) in &jobs.map {
        // A job in memory was attended from its record: it is loaded.
        let Some(other) = projects.peek(*other).filter(|p| p.id != project.id) else {
            continue;
        };
        if !other.phase().active() {
            continue;
        }
        let shares = supplies.chain(other.table).iter().any(|c| mine.contains(c));
        if !shares {
            continue;
        }
        if let Some(summary) = job.summary() {
            for (key, need) in &summary.bill {
                if let Some(have) = totals.get_mut(key) {
                    *have = have.saturating_sub(*need);
                }
            }
        }
    }
    totals
}

pub fn blueprint_tabled(
    content: &Content,
    projects: &Projects,
    id: ProjectId,
    table: [i32; 3],
) -> bool {
    get_block(table) == Some(content.table)
        && crate::table::blueprint_at(table).is_some_and(|stack| projects.bound(&stack) == Some(id))
}
