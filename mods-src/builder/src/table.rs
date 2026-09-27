mod actions;
mod blueprint;
mod materials;
mod panel;

use crate::host::prelude::*;

use crate::fx::HashMap;
use crate::jobs::Builder;
use crate::keys;
use crate::project::{Project, ProjectId};

pub use actions::{chosen, click, positioned, use_blueprint};
pub use blueprint::{blueprint_at, follow_blueprints, label_blueprint, placed, show_blueprint};

const PUBLISH: Cadence = Cadence::every(4);
const RESEND: Cadence = Cadence::every(40);
const REFUSAL_TICKS: u64 = 50;

#[derive(Default)]
pub struct Tables {
    laid: HashMap<[i32; 3], (bool, u64)>,
    refused: HashMap<(PlayerId, [i32; 3]), (crate::jobs::Refusal, u64)>,
}

impl Tables {
    pub fn sweep(&mut self, now: u64, idle: u64) {
        self.laid.retain(|_, (_, at)| now < *at + idle);
        self.refused.retain(|_, (_, until)| now < *until);
    }
}

fn project_at(builder: &Builder, table: [i32; 3]) -> Option<ProjectId> {
    if let Some(id) = builder.projects.active_at(table) {
        return Some(id);
    }
    match blueprint_at(table) {
        Some(blueprint) => builder.projects.bound(&blueprint),
        None => builder.projects.report_at(table),
    }
}

fn table_of(viewer: &GuiViewerData) -> Option<[i32; 3]> {
    match viewer.anchor {
        Some(ContainerAddress::Block(table))
            if viewer.kind == keys::table::KIND || viewer.kind == keys::materials::KIND =>
        {
            Some(table)
        }
        _ => None,
    }
}

pub fn watched_projects(builder: &Builder, viewers: &[GuiViewerData]) -> Vec<ProjectId> {
    let mut ids = Vec::new();
    for table in viewers.iter().filter_map(table_of) {
        if let Some(id) = project_at(builder, table).filter(|id| !ids.contains(id)) {
            ids.push(id);
        }
    }
    ids
}

fn may_edit(player: PlayerId, project: &Project) -> bool {
    player_identity(player).is_some_and(|who| who.operator || who.name == project.owner)
}

pub fn publish(builder: &mut Builder, now: u64, viewers: &[GuiViewerData]) {
    for viewer in viewers {
        let Some(table) = table_of(viewer) else {
            continue;
        };
        let stagger = u64::from(viewer.player_id.0);
        if !PUBLISH.due(now, stagger) && !builder.panels.is_fresh(viewer) {
            continue;
        }
        if RESEND.due(now, stagger) {
            builder.panels.resend(viewer);
        }
        publish_to(builder, viewer, table, now);
    }
}

fn publish_to(builder: &mut Builder, viewer: &GuiViewerData, table: [i32; 3], now: u64) {
    if viewer.kind == keys::table::KIND {
        show_blueprint(builder, table, now);
        let state = panel::describe(builder, viewer.player_id, table, now);
        builder.panels.publish(viewer, &state);
    } else {
        let state = materials::bill(builder, table, now);
        builder.panels.publish(viewer, &state);
    }
}

pub fn closed(builder: &mut Builder, closed: &[GuiViewerData], now: u64) {
    for viewer in closed {
        let Some(table) = table_of(viewer) else {
            continue;
        };
        builder.tables.refused.remove(&(viewer.player_id, table));
        if viewer.kind == keys::table::KIND {
            show_blueprint(builder, table, now);
        }
    }
}
