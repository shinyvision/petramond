//! The blueprint in a table's slot: shown lying on the table's top, bound to
//! its project, and labelled with what it builds.

use mod_sdk::*;

use crate::content::{INFO_DATA, PROJECT_DATA};
use crate::jobs::Builder;
use crate::project::ProjectId;

/// `petramond:info` values fit one instance-data entry.
const INFO_BYTES: usize = 64;
/// The table model's blueprint sheet and its paperweights: the row's `parts`,
/// in order.
const BLUEPRINT_PARTS: u32 = 0b1_1111;
/// How often a working project's table is looked at: the golem takes the
/// blueprint up and hands it back on its own.
const FOLLOW: Cadence = Cadence::every(10);

pub fn blueprint_at(table: [i32; 3]) -> Option<ItemStackData> {
    container_get(ContainerAddress::Block(table))?
        .into_iter()
        .next()
        .flatten()
}

/// Lay the blueprint out on the table's top while its slot holds one, and
/// clear it away when it does not. Said again only when it changes.
pub fn show_blueprint(builder: &mut Builder, table: [i32; 3], now: u64) {
    // A project outlives its table (broken, or gone from an older save), and
    // a part mask set on a block that is not this mod's is an error that
    // takes the whole mod down.
    if get_block(table) != Some(builder.content.table) {
        builder.tables.laid.remove(&table);
        return;
    }
    let laid = blueprint_at(table).is_some();
    if let Some((was, at)) = builder.tables.laid.get_mut(&table) {
        if *was == laid {
            *at = now;
            return;
        }
    }
    if set_model_parts(table, if laid { BLUEPRINT_PARTS } else { 0 }, None) {
        builder.tables.laid.insert(table, (laid, now));
    }
}

/// A table was placed: it starts with a bare top, whatever stood there
/// before.
pub fn placed(builder: &mut Builder, table: [i32; 3]) {
    builder.tables.laid.remove(&table);
}

/// Keep the tables of working projects showing what their slot holds.
pub fn follow_blueprints(builder: &mut Builder, now: u64) {
    let due: Vec<[i32; 3]> = builder
        .projects
        .live()
        .filter(|id| FOLLOW.due(now, *id))
        .filter_map(|id| builder.projects.peek(id))
        .filter(|p| p.phase().active())
        .map(|p| p.table)
        .collect();
    for table in due {
        show_blueprint(builder, table, now);
    }
}

pub(super) fn bind_blueprint(
    builder: &mut Builder,
    table: [i32; 3],
    mut blueprint: ItemStackData,
    id: ProjectId,
) {
    blueprint
        .data
        .retain(|(k, _)| k != PROJECT_DATA && k != INFO_DATA);
    blueprint
        .data
        .push((PROJECT_DATA.into(), builder.projects.binding(id)));
    blueprint.data.sort();
    container_set(ContainerAddress::Block(table), vec![(0, Some(blueprint))]);
}

pub fn label_blueprint(builder: &mut Builder, id: ProjectId) {
    let Some((table, mut title)) = builder.projects.get(id).map(|p| (p.table, p.title.clone()))
    else {
        return;
    };
    let Some(mut blueprint) = blueprint_at(table) else {
        return;
    };
    if builder.projects.bound(&blueprint) != Some(id) {
        return;
    }
    while title.len() > INFO_BYTES {
        title.pop();
    }
    blueprint.data.retain(|(k, _)| k != INFO_DATA);
    if !title.is_empty() {
        blueprint.data.push((INFO_DATA.into(), title.into_bytes()));
    }
    blueprint.data.sort();
    container_set(ContainerAddress::Block(table), vec![(0, Some(blueprint))]);
}
