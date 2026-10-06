//! `monsters:skeleton_post`: a guard keeps to its post. With nothing to fight, a guard that has
//! strayed past [`LEASH`] walks back until it is within [`ARRIVE`]; a watch guard stands on its
//! post. At its post with nothing to fight, a guard turns to the way its post faces. The state
//! lives in the mob's tags, so it saves and dies with the body.

use mod_sdk::*;

use super::keys::{FACING_TAG, POST_TAG, RETURNING_TAG, ROLE_TAG};
use super::posts::{standing_point, tagged_role};
use crate::post_marker::PostRole;

const LEASH: f64 = 6.0;
const ARRIVE: f64 = 1.2;
/// A guard this many cells above or below its post has left it (fallen off a wall, say).
const LEASH_DROP: i32 = 3;
/// A watch guard this far from its post's centre is off it.
const ON_POST: f64 = 0.6;

fn tag<'a>(ctx: &'a AiNodeCtx, key: &str) -> Option<&'a MobTagValue> {
    ctx.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

pub fn decide(ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    let post = tag(ctx, POST_TAG).and_then(<[i32; 3]>::from_tag)?;
    let watch = tag(ctx, ROLE_TAG).and_then(tagged_role) == Some(PostRole::Watch);
    let facing = match tag(ctx, FACING_TAG) {
        Some(MobTagValue::F64(yaw)) if yaw.is_finite() => Some(*yaw as f32),
        _ => None,
    };
    let returning = tag(ctx, RETURNING_TAG) == Some(&MobTagValue::Bool(true));
    let stand = standing_point(post);
    let goal = [post[0], post[1] + 1, post[2]];
    let across = (ctx.pos[0] - stand[0]).hypot(ctx.pos[2] - stand[2]);
    let drop = (ctx.cell[1] - goal[1]).abs();
    let at_post = drop == 0 && across <= ON_POST;
    let idle = ctx.target.is_none();

    if watch {
        let mut decision = AiNodeDecision {
            claims: ChannelClaims::of(&[DecisionChannel::Goal]),
            ..Default::default()
        };
        if at_post {
            decision.facing = facing.filter(|_| idle);
        } else {
            decision.goal = Some(goal);
        }
        return Some(decision);
    }

    let mut decision = AiNodeDecision::default();
    let stray = drop >= LEASH_DROP || across > LEASH;
    let home = drop <= 1 && across <= ARRIVE;
    let heading_home = idle && (returning || stray) && !home;
    if heading_home {
        decision.goal = Some(goal);
    } else if idle && at_post {
        decision.facing = facing;
    }
    if heading_home != returning {
        decision.tags.push(MobTagWrite {
            key: RETURNING_TAG.into(),
            value: heading_home.then_some(MobTagValue::Bool(true)),
        });
    }
    let silent = decision.goal.is_none() && decision.facing.is_none() && decision.tags.is_empty();
    (!silent).then_some(decision)
}
