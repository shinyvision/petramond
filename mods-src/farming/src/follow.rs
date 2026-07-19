//! The wheat lure: sheep follow a player holding wheat.
//!
//! A scripted AI node (`farming:follow_wheat`), composed onto the ENGINE
//! sheep through this pack's `mobs.json` `brain_extensions` row — the engine
//! knows nothing about wheat. The row DECLARES the facts this node reads
//! (`"inputs": ["player_held", "player_foothold"]`); undeclared facts never
//! reach `ctx`. Dispatch is detached and decision-only, so per-sheep state
//! rides the sheep's own TAG MAP: reads come in as `ctx.tags`, writes ride
//! `AiNodeDecision::tags` and are applied by the engine after the dispatch.
//! The state therefore persists, travels, and dies with the sheep — no
//! guest-side map, no `mob_died` release, no prune backstop.
//!
//! RULES: a sheep follows while the nearest player holds wheat within
//! [`FOLLOW_RADIUS`], but STOPS once inside [`STOP_RADIUS`] — it stands at
//! arm's length instead of crowding the player, resuming when the wheat
//! moves back out of reach. A followed player straying beyond the follow
//! radius breaks the follow AND makes the sheep refuse to re-follow for
//! 200–300 ticks; merely lowering the wheat ends the follow quietly with no
//! refusal. The goal emitted is the engine-computed foothold near the
//! player (the same cell `chase_player` targets), so the path is reachable
//! by construction.

use mod_sdk::*;

use crate::content::Content;

/// Follow engages — and holds — only within this distance (blocks, 3-D).
const FOLLOW_RADIUS: f32 = 8.0;
/// Close enough: inside this the sheep stands (goal = its own cell — which
/// also keeps wander from strolling it away mid-lure) instead of pressing
/// into the player.
const STOP_RADIUS: f32 = 3.0;
/// Re-follow refusal after a broken follow: 200 + (0..=100) ticks.
const SULK_MIN: u64 = 200;
const SULK_SPAN: u64 = 101;

/// `Bool(true)` while the sheep is actively following the lure.
const FOLLOWING: &str = "farming:following";
/// `Int` absolute game tick the re-follow refusal holds until (`ctx.tick`
/// based, so a sulk keeps its real duration even across skipped dispatches —
/// and now across save/reload too, since tags persist).
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

/// A decision that only carries tag writes (no goal or other opinion) — or
/// `None` when there is nothing to write either.
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
    // Whatever happens next, an expired sulk tag is stale — retire it with
    // the transition it accompanies.
    let expired_sulk = sulk.map(|_| delete(SULK_UNTIL));
    if ctx.player_held != Some(content.wheat_item) {
        // Wheat lowered (or an expired sulk with no lure): the follow ends
        // quietly and the state retires.
        let mut tags: Vec<MobTagWrite> = expired_sulk.into_iter().collect();
        if following {
            tags.push(delete(FOLLOWING));
        }
        return tags_only(tags);
    }
    let [dx, dy, dz] = [
        ctx.player_pos[0] - ctx.pos[0],
        ctx.player_pos[1] - ctx.pos[1],
        ctx.player_pos[2] - ctx.pos[2],
    ];
    let dist2 = dx * dx + dy * dy + dz * dz;
    if dist2 > FOLLOW_RADIUS * FOLLOW_RADIUS {
        if following {
            // The lure walked off: the follow breaks and the sheep refuses
            // to re-engage for a deterministic per-break roll.
            let until = now + SULK_MIN + rng_u64("follow_sulk") % SULK_SPAN;
            return tags_only(vec![
                delete(FOLLOWING),
                set(SULK_UNTIL, MobTagValue::I64(until as i64)),
            ]);
        }
        // Expired sulk, lure out of range: nothing left to remember.
        return tags_only(expired_sulk.into_iter().collect());
    }
    // Engagement is the only insert on the held-wheat path.
    let mut tags: Vec<MobTagWrite> = expired_sulk.into_iter().collect();
    if !following {
        tags.push(set(FOLLOWING, MobTagValue::Bool(true)));
    }
    if dist2 <= STOP_RADIUS * STOP_RADIUS {
        // Close enough — stand attentively at the lure until it moves.
        return Some(AiNodeDecision {
            goal: Some(ctx.cell),
            tags,
            ..Default::default()
        });
    }
    // No reachable foothold (airborne player) = no goal this tick; the
    // follow itself holds, like chase_player's airborne fall-through.
    // The engagement tag still lands even goalless.
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
