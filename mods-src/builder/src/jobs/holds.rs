use crate::host::prelude::*;

use crate::jobs::{Builder, Refusal, TABLE_CHECK};
use crate::project::{Brief, Hold, Note, Phase, ProjectId};
use crate::supplies;

impl Builder {
    pub fn pause(&mut self, id: ProjectId) {
        self.projects.update(id, |p| {
            if p.phase().active() && p.hold().is_none() {
                p.hold_for(Hold::Player, Note::Paused);
            }
        });
    }

    pub fn resume(&mut self, id: ProjectId, table: [i32; 3]) -> Result<(), Refusal> {
        let project = self
            .projects
            .get(id)
            .map(|p| p.brief())
            .ok_or(Refusal::NoProject)?;
        match project.hold {
            None => return Ok(()),
            Some(Hold::Worker) => return self.start(id),
            Some(Hold::Supplies) | Some(Hold::Table) => {
                self.admission(&project.at(table), current_tick())?;
            }
            Some(Hold::Blueprint) => return Err(Refusal::GolemLostBlueprint),
            Some(Hold::Player) | Some(Hold::Storage) => {}
        }
        self.projects.update(id, |p| {
            p.table = table;
            p.release();
            p.note = Note::None;
        });
        Ok(())
    }

    pub(super) fn check_supplies(&mut self, project: &Brief, now: u64) {
        if project.phase != Phase::Working || matches!(project.hold, Some(Hold::Player)) {
            return;
        }
        if project.hold == Some(Hold::Storage) {
            let stock = self.supplies.stock_at(project.table, now);
            if stock.read && stock.has_room() {
                self.projects.update(project.id, |p| {
                    p.release();
                    if p.note == Note::ChestsFull {
                        p.note = Note::None;
                    }
                });
            }
            return;
        }
        let Some(summary) = self.jobs.map.get(&project.id).and_then(|job| job.summary()) else {
            return;
        };
        let have = self.available(project, now);
        let short = supplies::shortfall(&summary.bill, &have);
        let short = supplies::worst(&short, &mut self.caches);
        if short.is_some() && !self.supplies.stock_at(project.table, now).read {
            return;
        }
        self.projects.update(project.id, |p| match &short {
            Some(short) => p.note = Note::Missing(short.clone()),
            None => {
                if p.hold() == Some(Hold::Supplies) {
                    p.release();
                }
                if p.note.is_shortfall() {
                    p.note = Note::None;
                }
            }
        });
    }

    pub fn died(&mut self, mob: u64) {
        let Some(job) = self.jobs.by_mob(mob) else {
            return;
        };
        job.crew = Default::default();
        let id = job.id;
        self.projects
            .update(id, crate::project::Project::golem_died);
    }

    pub(super) fn cancel_tableless(&mut self, live: &[ProjectId], now: u64) {
        for &id in live.iter().filter(|id| TABLE_CHECK.due(now, **id)) {
            let gone = self.projects.get(id).is_some_and(|p| {
                !p.cancelling() && get_block(p.table).is_some_and(|b| b != self.content.table)
            });
            if gone {
                self.cancel(id);
                self.projects.update(id, |p| p.note = Note::TableGone);
            }
        }
    }

    pub fn cancel(&mut self, id: ProjectId) {
        self.projects.update(id, crate::project::Project::cancel);
    }
}
