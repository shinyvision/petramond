//! The wheat lure: sheep follow a player holding wheat.
//!
//! A scripted AI node (`farming:follow_wheat`), composed onto the ENGINE
//! sheep through this pack's `mobs.json` `brain_extensions` row — the engine
//! knows nothing about wheat. The row DECLARES the facts this node reads
//! (`"inputs": ["player_held", "player_foothold"]`); undeclared facts never
//! reach `ctx`. Dispatch is detached and decision-only, so all per-mob state
//! lives here, keyed by the stable mob id; time is `ctx.tick`.
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

use std::collections::HashMap;

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
/// Backstop bound, not the release mechanism — `mob_died` releases entries;
/// this catches sheep that left the world without dying (section unload).
const PRUNE_LEN: usize = 1024;

/// Per-sheep lure state, keyed by stable mob id. An entry exists ONLY while
/// the sheep is actively following or sulking; the idle 99% of dispatches
/// touch the map read-only and insert nothing.
#[derive(Default)]
pub struct Follow {
    mobs: HashMap<u64, Sheep>,
}

#[derive(Copy, Clone, Default)]
struct Sheep {
    following: bool,
    /// Re-follow refusal holds until this game tick (`ctx.tick`-based, so a
    /// sulk keeps its real duration even across skipped dispatches).
    sulk_until: u64,
}

/// Release a dead sheep's entry (the `mob_died` handler's hook).
pub fn release(state: &mut Follow, mob_id: u64) {
    state.mobs.remove(&mob_id);
}

pub fn decide(content: &Content, state: &mut Follow, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    let now = ctx.tick;
    if state.mobs.len() > PRUNE_LEN {
        state.mobs.retain(|_, s| s.sulk_until > now);
    }
    let prior = state.mobs.get(&ctx.mob_id).copied();
    if prior.is_some_and(|s| now < s.sulk_until) {
        return None;
    }
    let following = prior.is_some_and(|s| s.following);
    if ctx.player_held != Some(content.wheat_item) {
        // Wheat lowered (or an expired sulk with no lure): the follow ends
        // quietly and the entry retires.
        if prior.is_some() {
            state.mobs.remove(&ctx.mob_id);
        }
        return None;
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
            state.mobs.insert(
                ctx.mob_id,
                Sheep {
                    following: false,
                    sulk_until: now + SULK_MIN + rng_u64("follow_sulk") % SULK_SPAN,
                },
            );
        } else if prior.is_some() {
            // Expired sulk, lure out of range: nothing left to remember.
            state.mobs.remove(&ctx.mob_id);
        }
        return None;
    }
    if !following {
        // Engagement is the only insert on the held-wheat path.
        state.mobs.insert(
            ctx.mob_id,
            Sheep {
                following: true,
                sulk_until: 0,
            },
        );
    }
    if dist2 <= STOP_RADIUS * STOP_RADIUS {
        // Close enough — stand attentively at the lure until it moves.
        return Some(AiNodeDecision {
            goal: Some(ctx.cell),
            ..Default::default()
        });
    }
    // No reachable foothold (airborne player) = no goal this tick; the
    // follow itself holds, like chase_player's airborne fall-through.
    let goal = ctx.player_foothold?;
    Some(AiNodeDecision {
        goal: Some(goal),
        ..Default::default()
    })
}
