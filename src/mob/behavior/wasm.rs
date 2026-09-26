//! The scripted (WASM) AI node every namespaced (`mod_id:name`) brain-row
//! `node` key resolves to.
//!
//! Unlike the block-behavior hooks (fire-and-forget after the world tick), a
//! node's decision feeds the brain's priority arbitration the SAME tick, so
//! it is dispatched before the brains decide: the mob manager asks every
//! claimed [`ScriptedNode`] for its [`AiNodeRequest`] (the mob's `AiCtx`
//! snapshotted into ABI vocabulary, its tag map by shared handle), hands the
//! whole population's requests to `modding::ai::dispatch_batch` — one guest
//! call per node key — and each mob's replies ride its `AiCtx::scripted`
//! into [`WasmNodeAi::tick`], which converts them like any engine node's
//! output. Detached — no sim scope, decision-only (see `GuestCall::AiNode`).
//! No registration (mod disabled, key unclaimed) means no opinion, exactly
//! like an engine node returning defaults, and no request is built.
//!
//! Perception FACTS beyond the always-present baseline are PULL-model: the
//! brain row DECLARES the facts its node reads (`"inputs": ["player_held"]`
//! in `mobs.json`, parsed into [`ScriptedInputs`] at load), and only declared
//! facts are computed and shipped. Adding a fact = a [`ScriptedInputs`] flag,
//! a compute arm here, and an `AiNodeCtx` field — undeclaring mobs never pay
//! for it, and an unclaimed key computes nothing at all.

use crate::modding::ai::AiNodeRequest;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput, HeadLook};
use super::super::EntityRef;
pub use petramond_world::ai_vocab::ScriptedInputs;

use petramond_math::math::IVec3;

/// A scripted node's identity: the registry key its brain row names and the
/// facts the row declared.
pub struct ScriptedNode {
    key: &'static str,
    inputs: ScriptedInputs,
}

impl ScriptedNode {
    /// Whether a mod claims this node's key on this thread.
    fn claimed(&self) -> bool {
        crate::modding::ai::is_claimed(self.key)
    }

    /// This node's request for the mob `ctx` describes, or `None` when no mod
    /// claims the key — then nothing is computed and the node has no opinion.
    pub fn request(&self, ctx: &AiCtx) -> Option<AiNodeRequest> {
        if !self.claimed() {
            return None;
        }
        Some(AiNodeRequest {
            key: self.key,
            mob_id: ctx.mob_id,
            pos: ctx.pos.to_array(),
            cell: ctx.cell.to_array(),
            yaw: ctx.yaw,
            player_id: mod_api::PlayerId(ctx.player_id.0),
            player_pos: ctx.player_pos.to_array(),
            nav_idle: ctx.nav_idle,
            in_fluid: ctx.in_fluid.map(|b| mod_api::BlockId(b.id())),
            target: ctx.target.map(abi_entity),
            attacker: ctx.attacker.map(|(who, age)| (abi_entity(who), age)),
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
                    <= f64::from(
                        crate::mob::PLAYER_REACTIVE_RANGE * crate::mob::PLAYER_REACTIVE_RANGE,
                    ))
            .then(|| super::chase::goal_cell_near(ctx, ctx.player_pos))
            .flatten()
            .map(|c| c.to_array()),
            // The mob's own tag map — baseline own-state, so a node persists
            // per-mob state through decision tag writes instead of keying a
            // guest-side map off mob_id. Shared, not copied.
            tags: std::sync::Arc::clone(ctx.tags),
        })
    }
}

pub struct WasmNodeAi {
    node: ScriptedNode,
}

impl WasmNodeAi {
    pub(super) fn new(key: &'static str, inputs: ScriptedInputs) -> Self {
        WasmNodeAi {
            node: ScriptedNode { key, inputs },
        }
    }
}

impl AiBehavior for WasmNodeAi {
    fn scripted(&self) -> Option<&ScriptedNode> {
        Some(&self.node)
    }

    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        // An unclaimed key had no request gathered, so it owns no reply slot:
        // the claimed set cannot change between the gather and this decide
        // (both run on the sim thread within one tick).
        if !self.node.claimed() {
            return BehaviorOutput::default();
        }
        let Some(d) = ctx.scripted.take_next() else {
            return BehaviorOutput::default();
        };
        // Every channel an engine node fills, converted 1:1. A scripted strike
        // lands on the decision's own target, else on the brain's current lock
        // — the `melee_attack` rule; no target, no strike.
        let target = d.target.map(engine_entity);
        BehaviorOutput {
            goal: d.goal.map(IVec3::from),
            head_look: d.head_look.map(|[yaw, pitch]| HeadLook { yaw, pitch }),
            facing: d.facing.filter(|angle| angle.is_finite()),
            speed_scale: d.speed_scale.filter(|scale| scale.is_finite()),
            idle_anim: d.idle_anim,
            attack: d.attack.and_then(|[damage, knockback]| {
                target.or(ctx.target).map(|target| AttackIntent {
                    target,
                    damage,
                    knockback,
                })
            }),
            animation: d.animation.filter(|name| {
                let ok = crate::mob::anim::valid_clip_name(name);
                if !ok {
                    log::warn!(
                        "AI node '{}' decision animation {name:?} is empty or over {} bytes — dropped",
                        self.node.key,
                        mod_api::MAX_MOB_ANIM_NAME_BYTES
                    );
                }
                ok
            }),
            target,
            claims: d.claims,
            tag_writes: self.convert_tag_writes(d.tags),
        }
    }
}

fn abi_entity(who: EntityRef) -> mod_api::EntityRef {
    match who {
        EntityRef::Player(id) => mod_api::EntityRef::Player(mod_api::PlayerId(id.0)),
        EntityRef::Mob(id) => mod_api::EntityRef::Mob(id),
    }
}

fn engine_entity(who: mod_api::EntityRef) -> EntityRef {
    match who {
        mod_api::EntityRef::Player(id) => EntityRef::Player(crate::player::PlayerId(id.0)),
        mod_api::EntityRef::Mob(id) => EntityRef::Mob(id),
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
        let own = petramond_world::registry::namespace(self.node.key).unwrap_or("");
        writes
            .into_iter()
            .filter(|w| {
                let ok =
                    petramond_world::registry::namespace(&w.key) == Some(own) && !own.is_empty();
                if !ok {
                    log::warn!(
                        "AI node '{}' decision tag write '{}' outside its own namespace — dropped",
                        self.node.key,
                        w.key
                    );
                }
                ok
            })
            .map(|w| (w.key, w.value.map(crate::mob::MobTagValue::from)))
            .collect()
    }
}
