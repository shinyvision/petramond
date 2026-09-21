//! `builder:worker`: the golem's steering node. It holds no state and makes
//! no calls — it turns the goal, hold, facing and gaze the job leaves in the
//! golem's tags into this tick's decision.

use mod_sdk::*;

use crate::geometry::{decode_cell, decode_point};
use crate::worker::{EYE_HEIGHT as EYE, FACE_TAG, GOAL_TAG, HOLD_TAG, LOOK_TAG, PROJECT_TAG};

/// How far the neck turns and tilts: down far enough to see the block under
/// its own feet.
const NECK_YAW: f32 = 1.3;
const NECK_PITCH: f32 = 1.55;

pub fn decide(ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    let tag = |key: &str| ctx.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    tag(PROJECT_TAG)?;
    let hold = matches!(tag(HOLD_TAG), Some(MobTagValue::Bool(true)));
    let goal = match tag(GOAL_TAG) {
        Some(MobTagValue::Str(text)) if !hold => decode_cell(text),
        _ => None,
    };
    let facing = match tag(FACE_TAG) {
        Some(MobTagValue::F64(yaw)) => Some(*yaw as f32),
        _ => None,
    };
    let head_look = match tag(LOOK_TAG) {
        Some(MobTagValue::Str(text)) => decode_point(text).map(|point| gaze(ctx, point)),
        _ => None,
    };
    let mut claims = vec![DecisionChannel::Goal, DecisionChannel::Facing];
    if head_look.is_some() {
        claims.push(DecisionChannel::HeadLook);
    }
    Some(AiNodeDecision {
        goal: goal.or((!hold).then_some(ctx.cell)),
        facing,
        head_look,
        claims: ChannelClaims::of(&claims),
        ..Default::default()
    })
}

/// The head's turn and tilt toward `point`, relative to the body. Past what
/// the neck turns the head leads as far as it goes while the body comes round.
fn gaze(ctx: &AiNodeCtx, point: [f64; 3]) -> [f32; 2] {
    let to = [
        point[0] - ctx.pos[0],
        point[1] - (ctx.pos[1] + EYE),
        point[2] - ctx.pos[2],
    ];
    let horizontal = (to[0] * to[0] + to[2] * to[2]).sqrt();
    let yaw = if horizontal < 0.02 {
        0.0
    } else {
        let bearing = (-to[0]).atan2(-to[2]) as f32;
        ((bearing - ctx.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI)
            .clamp(-NECK_YAW, NECK_YAW)
    };
    let pitch = (to[1].atan2(horizontal.max(1e-3)) as f32).clamp(-NECK_PITCH, NECK_PITCH);
    [yaw, pitch]
}
