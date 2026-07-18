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
use crate::mathh::IVec3;

/// The declarable scripted-node input facts a brain row may request. Engine
/// nodes read `AiCtx` directly and declare none (the loader rejects `inputs`
/// on them); only the scripted node ships facts across the ABI.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScriptedInputs {
    /// The nearest player's selected (held) item.
    pub player_held: bool,
    /// The engine foothold scan near the nearest player (`chase_player`'s
    /// goal cell) — the expensive multi-cell probe.
    pub player_foothold: bool,
}

impl ScriptedInputs {
    /// The declarable input names, in `mobs.json` vocabulary.
    const KNOWN: &'static [&'static str] = &["player_held", "player_foothold"];

    /// Parse a brain row's `inputs` list. Unknown names are errors (a typo'd
    /// fact must fail the load, not silently read `None` forever).
    pub fn parse(names: &[String]) -> Result<Self, String> {
        let mut inputs = ScriptedInputs::default();
        for name in names {
            match name.as_str() {
                "player_held" => inputs.player_held = true,
                "player_foothold" => inputs.player_foothold = true,
                other => {
                    return Err(format!(
                        "unknown input '{other}' (declarable inputs: {})",
                        Self::KNOWN.join(", ")
                    ));
                }
            }
        }
        Ok(inputs)
    }

    pub fn is_empty(self) -> bool {
        self == ScriptedInputs::default()
    }
}

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
        }
    }
}
