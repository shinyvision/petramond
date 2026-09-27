use crate::host::prelude::*;

use super::{Body, Ctx, FULL_HEALTH_TAG, HEALTH_TAG};
use crate::project::{Hold, Note, Project, Projects};

pub(super) fn open_block(ctx: &mut Ctx, block: BlockId) -> bool {
    ctx.caches
        .block(block)
        .is_some_and(|info| info.collision.is_empty() && info.fluid.is_none())
}

pub(super) fn hold_for_blueprint(projects: &mut Projects, project: &Project, body: &Body) {
    let carried = body
        .slots
        .iter()
        .flatten()
        .any(|stack| projects.bound(stack) == Some(project.id));
    match (carried, project.hold()) {
        (false, None) => {
            projects.update(project.id, |p| {
                p.hold_for(Hold::Blueprint, Note::MissingBlueprint)
            });
        }
        (true, Some(Hold::Blueprint)) => {
            projects.update(project.id, |p| {
                p.release();
                p.note = Note::None;
            });
        }
        _ => {}
    }
}

pub(super) fn mend(id: u64, health: f32) {
    let MobTagLookup::Value(MobTagValue::F64(full)) = mob_tag_get(id, FULL_HEALTH_TAG) else {
        return;
    };
    if f64::from(health) < full {
        mob_tag_set(
            id,
            HEALTH_TAG,
            MobTagValue::F64((f64::from(health) + 1.0).min(full)),
        );
    }
}
