//! Wheat lure. Sheep follow whoever's nearest holding wheat.
//!
//! Scripted AI node, added to engine sheep by this pack's `brain_extensions` row.
//! The engine has no idea wheat exists. The node only gets the facts the row declares,
//! and it keeps per-sheep state in the sheep's tags, so that dies with the sheep.
//!
//! Sheep follow inside [`FOLLOW_RADIUS`], stop at [`STOP_RADIUS`] and resume past
//! [`RESUME_RADIUS`]. Walk off past follow range and a sheep won't follow you again
//! for 200-300 ticks. Just lowering the wheat ends it with no refusal. The goal is the
//! foothold `chase_player` uses, so the path's always reachable.

use mod_sdk::*;

use crate::content::Content;

const FOLLOW_RADIUS: f32 = 8.0;
const STOP_RADIUS: f32 = 3.0;
const RESUME_RADIUS: f32 = 4.0;
const SULK_MIN: u64 = 200;
const SULK_SPAN: u64 = 101;

const FOLLOWING: &str = "farming:following";
const NEAR: &str = "farming:follow_near";
const SULK_UNTIL: &str = "farming:sulk_until";

fn tag_bool(ctx: &AiNodeCtx, key: &str) -> bool {
    ctx.tags
        .iter()
        .any(|(k, v)| k == key && *v == MobTagValue::Bool(true))
}

fn tag_int(ctx: &AiNodeCtx, key: &str) -> Option<i64> {
    ctx.tags.iter().find_map(|(k, v)| match v {
        MobTagValue::I64(i) if k == key => Some(*i),
        _ => None,
    })
}

fn set(key: &str, value: MobTagValue) -> MobTagWrite {
    MobTagWrite {
        key: key.into(),
        value: Some(value),
    }
}

fn delete(key: &str) -> MobTagWrite {
    MobTagWrite {
        key: key.into(),
        value: None,
    }
}

fn tags_only(tags: Vec<MobTagWrite>) -> Option<AiNodeDecision> {
    if tags.is_empty() {
        return None;
    }
    Some(AiNodeDecision {
        tags,
        ..Default::default()
    })
}

pub fn decide(content: &Content, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    let now = ctx.tick;
    let sulk = tag_int(ctx, SULK_UNTIL);
    if sulk.is_some_and(|until| now < until as u64) {
        return None;
    }
    let following = tag_bool(ctx, FOLLOWING);
    let expired_sulk = sulk.map(|_| delete(SULK_UNTIL));
    if ctx.player_held != Some(content.wheat_item) {
        let mut tags: Vec<MobTagWrite> = expired_sulk.into_iter().collect();
        if following {
            tags.push(delete(FOLLOWING));
        }
        if tag_bool(ctx, NEAR) {
            tags.push(delete(NEAR));
        }
        return tags_only(tags);
    }
    let [dx, dy, dz] = [
        (ctx.player_pos[0] - ctx.pos[0]) as f32,
        (ctx.player_pos[1] - ctx.pos[1]) as f32,
        (ctx.player_pos[2] - ctx.pos[2]) as f32,
    ];
    let dist2 = dx * dx + dy * dy + dz * dz;
    if dist2 > FOLLOW_RADIUS * FOLLOW_RADIUS {
        if following {
            let until = now + SULK_MIN + rng_u64("follow_sulk") % SULK_SPAN;
            let mut tags = vec![
                delete(FOLLOWING),
                set(SULK_UNTIL, MobTagValue::I64(until as i64)),
            ];
            if tag_bool(ctx, NEAR) {
                tags.push(delete(NEAR));
            }
            return tags_only(tags);
        }
        return tags_only(expired_sulk.into_iter().collect());
    }
    let mut tags: Vec<MobTagWrite> = expired_sulk.into_iter().collect();
    if !following {
        tags.push(set(FOLLOWING, MobTagValue::Bool(true)));
    }
    let was_near = tag_bool(ctx, NEAR);
    let near = if was_near {
        dist2 <= RESUME_RADIUS * RESUME_RADIUS
    } else {
        dist2 <= STOP_RADIUS * STOP_RADIUS
    };
    if near != was_near {
        tags.push(if near {
            set(NEAR, MobTagValue::Bool(true))
        } else {
            delete(NEAR)
        });
    }
    if near {
        return Some(AiNodeDecision {
            goal: Some(ctx.cell),
            tags,
            ..Default::default()
        });
    }
    let goal = ctx.player_foothold;
    if goal.is_none() && tags.is_empty() {
        return None;
    }
    Some(AiNodeDecision {
        goal,
        tags,
        ..Default::default()
    })
}
