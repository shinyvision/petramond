//! The schematic table: its blueprint slot, the panel each viewer sees, the
//! materials page, and the buttons that drive a project.
//!
//! - [`panel`] — the table panel's state and what it says.
//! - [`materials`] — the materials page's bill.
//! - [`actions`] — the buttons, and what a schematic choice or positioning
//!   reports back.
//! - [`blueprint`] — the blueprint in the slot: laid out on the model, bound
//!   and labelled.

mod actions;
mod blueprint;
mod materials;
mod panel;

use mod_sdk::*;

use crate::content::{MATERIALS_KIND, TABLE_KIND};
use crate::fx::HashMap;
use crate::jobs::Builder;
use crate::project::{Project, ProjectId};

pub use actions::{chosen, click, positioned, use_blueprint};
pub use blueprint::{blueprint_at, follow_blueprints, label_blueprint, placed, show_blueprint};
pub use panel::Tone;

/// Panels refresh this often, each viewer on a tick of their own.
const PUBLISH: Cadence = Cadence::every(4);
/// Every key is sent again this often, changed or not: a state map emptied
/// without the panel reading as closed would otherwise stay empty.
const RESEND: Cadence = Cadence::every(40);
/// How long a refused press stays on the status line (ticks).
const REFUSAL_TICKS: u64 = 50;

/// What the session remembers per table.
#[derive(Default)]
pub struct Tables {
    /// Whether a blueprint was last laid out on each table's top, and the
    /// tick that was last checked.
    laid: HashMap<[i32; 3], (bool, u64)>,
    /// The press each viewer was last refused, and the tick it stops showing.
    refused: HashMap<(PlayerId, [i32; 3]), (String, u64)>,
}

impl Tables {
    /// Let go of tables nobody has looked at for `idle` ticks.
    pub fn sweep(&mut self, now: u64, idle: u64) {
        self.laid.retain(|_, (_, at)| now < *at + idle);
        self.refused.retain(|_, (_, until)| now < *until);
    }
}

/// The project a table speaks for: the job working from it (its golem
/// carries the blueprint), else the one its blueprint is bound to, else —
/// with the slot empty — the last job that finished here, for its report.
fn project_at(builder: &Builder, table: [i32; 3]) -> Option<ProjectId> {
    if let Some(id) = builder.projects.at_table(table, |p| p.phase().active()) {
        return Some(id);
    }
    match blueprint_at(table) {
        Some(blueprint) => builder.projects.bound(&blueprint),
        None => builder.projects.at_table(table, |p| p.phase().finished()),
    }
}

fn table_of(viewer: &GuiViewerData) -> Option<[i32; 3]> {
    match viewer.anchor {
        Some(ContainerAddress::Block(table))
            if viewer.kind == TABLE_KIND || viewer.kind == MATERIALS_KIND =>
        {
            Some(table)
        }
        _ => None,
    }
}

/// Projects whose table panel someone has open this tick.
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
    if viewer.kind == TABLE_KIND {
        show_blueprint(builder, table, now);
        let state = panel::describe(builder, viewer.player_id, table, now);
        builder.panels.publish(viewer, &state);
    } else {
        let state = materials::bill(builder, table, now);
        builder.panels.publish(viewer, &state);
    }
}

/// Panels that closed since last tick. A panel closing is the last moment
/// its slot could have changed.
pub fn closed(builder: &mut Builder, closed: &[GuiViewerData], now: u64) {
    for viewer in closed {
        let Some(table) = table_of(viewer) else {
            continue;
        };
        builder.tables.refused.remove(&(viewer.player_id, table));
        if viewer.kind == TABLE_KIND {
            show_blueprint(builder, table, now);
        }
    }
}
