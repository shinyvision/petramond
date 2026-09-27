use crate::host::prelude::*;

use crate::content::{INFO_DATA, PROJECT_DATA};
use crate::jobs::Builder;
use crate::project::ProjectId;

const INFO_BYTES: usize = 64;
const BLUEPRINT_PARTS: u32 = 0b1_1111;
const FOLLOW: Cadence = Cadence::every(10);

pub fn blueprint_at(table: [i32; 3]) -> Option<ItemStackData> {
    container_get(ContainerAddress::Block(table))?
        .into_iter()
        .next()
        .flatten()
}

pub fn show_blueprint(builder: &mut Builder, table: [i32; 3], now: u64) {
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

pub fn placed(builder: &mut Builder, table: [i32; 3]) {
    builder.tables.laid.remove(&table);
}

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
