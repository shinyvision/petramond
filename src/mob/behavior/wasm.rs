//! The scripted (WASM) AI node every namespaced (`mod_id:name`) brain-row
//! `node` key resolves to.
//!
//! Unlike the block-behavior hooks (fire-and-forget after the world tick), a
//! node's decision feeds the brain's priority arbitration NOW, so the
//! dispatch is synchronous: `tick` snapshots the [`AiCtx`] into the ABI's
//! `AiNodeCtx` and calls the owning mod through the main-thread registry
//! (`modding::ai`), detached — no sim scope, decision-only (see
//! `GuestCall::AiNode`). No registration (mod disabled, key unclaimed) means
//! no opinion, exactly like an engine node returning defaults.
//!
//! Perception FACTS beyond the always-present baseline are PULL-model: the
//! brain row DECLARES the facts its node reads (`"inputs": ["player_held"]`
//! in `mobs.json`, parsed into [`ScriptedInputs`] at load), and only declared
//! facts are computed and shipped. Adding a fact = a [`ScriptedInputs`] flag,
//! a compute arm here, and an `AiNodeCtx` field — undeclaring mobs never pay
//! for it, and an unclaimed key computes nothing at all.

use mod_api::AiNodeCtx;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput, HeadLook};
pub use petramond_world::ai_vocab::ScriptedInputs;

use petramond_math::math::IVec3;

pub struct WasmNodeAi {
    key: &'static str,
    inputs: ScriptedInputs,
}

impl WasmNodeAi {
    pub(super) fn new(key: &'static str, inputs: ScriptedInputs) -> Self {
        WasmNodeAi { key, inputs }
    }
}

impl AiBehavior for WasmNodeAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        // Unclaimed key (mod disabled / never registered) = no opinion, and
        // no snapshot either — the declared facts are computed only for a
        // dispatch that will actually happen.
        if !crate::modding::ai::is_claimed(self.key) {
            return BehaviorOutput::default();
        }
        let snapshot = AiNodeCtx {
            mob_id: ctx.mob_id,
            pos: ctx.pos.to_array(),
            cell: ctx.cell.to_array(),
            yaw: ctx.yaw,
            tick: ctx.world.current_tick(),
            player_id: mod_api::PlayerId(ctx.player_id.0),
            player_pos: ctx.player_pos.to_array(),
            nav_idle: ctx.nav_idle,
            in_water: ctx.in_water,
            player_held: (self.inputs.player_held)
                .then_some(ctx.player_held)
                .flatten()
                .map(|i| mod_api::ItemId(i.id())),
            // The engine-side foothold scan (what chase_player targets), so
            // a scripted follow node emits reachable goals without world
            // access of its own. Distance-gated even when declared: past the
            // range where mob AI reacts to players at all, a foothold goal
            // is useless and the cells stay unread.
            player_foothold: (self.inputs.player_foothold
                && ctx.pos.distance_squared(ctx.player_pos)
                    <= crate::mob::PLAYER_REACTIVE_RANGE * crate::mob::PLAYER_REACTIVE_RANGE)
                .then(|| super::chase::goal_cell_near(ctx, ctx.player_pos))
                .flatten()
                .map(|c| c.to_array()),
            // The mob's own tag map — baseline own-state, so a node persists
            // per-mob state through decision tag writes instead of keying a
            // guest-side map off mob_id.
            tags: ctx
                .tags
                .iter()
                .map(|(k, v)| (k.clone(), mod_api::MobTagValue::from(v)))
                .collect(),
        };
        let Some(d) = crate::modding::ai::dispatch(self.key, &snapshot) else {
            return BehaviorOutput::default();
        };
        BehaviorOutput {
            goal: d.goal.map(IVec3::from),
            head_look: d.head_look.map(|[yaw, pitch]| HeadLook { yaw, pitch }),
            idle_anim: d.idle_anim,
            // A scripted strike targets the nearest player — the only target
            // the single-player-shaped AI-node ABI can express today.
            attack: d.attack.map(|[damage, knockback]| AttackIntent {
                target: crate::mob::EntityRef::Player(ctx.player_id),
                damage,
                knockback,
            }),
            target: None,
            tag_writes: self.convert_tag_writes(d.tags),
        }
    }
}

impl WasmNodeAi {
    /// Validate and convert a decision's tag writes: a decision may only
    /// write keys in ITS OWN mod's namespace (stricter than the `MobTagSet`
    /// HostCall, which also accepts exposed `petramond:*` keys). A violating
    /// write is dropped with a warning, never applied — the decision's other
    /// fields still count.
    fn convert_tag_writes(
        &self,
        writes: Vec<mod_api::MobTagWrite>,
    ) -> Vec<(String, Option<crate::mob::MobTagValue>)> {
        if writes.is_empty() {
            return Vec::new();
        }
        let own = petramond_world::registry::namespace(self.key).unwrap_or("");
        writes
            .into_iter()
            .filter(|w| {
                let ok = petramond_world::registry::namespace(&w.key) == Some(own) && !own.is_empty();
                if !ok {
                    log::warn!(
                        "AI node '{}' decision tag write '{}' outside its own namespace — dropped",
                        self.key,
                        w.key
                    );
                }
                ok
            })
            .map(|w| (w.key, w.value.map(crate::mob::MobTagValue::from)))
            .collect()
    }
}
