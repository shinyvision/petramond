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

use mod_api::AiNodeCtx;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput, HeadLook};
use crate::mathh::IVec3;

pub struct WasmNodeAi {
    key: &'static str,
}

impl WasmNodeAi {
    pub(super) fn new(key: &'static str) -> Self {
        WasmNodeAi { key }
    }
}

impl AiBehavior for WasmNodeAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let snapshot = AiNodeCtx {
            mob_id: ctx.mob_id,
            pos: ctx.pos.to_array(),
            cell: ctx.cell.to_array(),
            yaw: ctx.yaw,
            player_pos: ctx.player_pos.to_array(),
            nav_idle: ctx.nav_idle,
            in_water: ctx.in_water,
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
