//! Looking after the golem itself: its health, its blueprint, and what
//! counts as open room for it.

use mod_sdk::*;

use super::{Body, Ctx, FULL_HEALTH_TAG, HEALTH_TAG};
use crate::project::{Hold, Project, Projects};

/// Whether a cell holding `block` leaves room for a body, a scaffold or a
/// line of sight.
pub(super) fn open_block(ctx: &mut Ctx, block: BlockId) -> bool {
    ctx.caches
        .block(block)
        .is_some_and(|info| info.collision.is_empty() && info.fluid.is_none())
}

/// A working golem carries its project's blueprint: without it the job is
/// held, and released when the blueprint is back in its hands.
pub(super) fn hold_for_blueprint(projects: &mut Projects, project: &Project, body: &Body) {
    let carried = body
        .slots
        .iter()
        .flatten()
        .any(|stack| projects.bound(stack) == Some(project.id));
    match (carried, project.hold()) {
        (false, None) => {
            projects.update(project.id, |p| {
                p.hold_for(Hold::Blueprint, "Missing blueprint")
            });
        }
        (true, Some(Hold::Blueprint)) => {
            projects.update(project.id, |p| {
                p.release();
                p.note.clear();
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
