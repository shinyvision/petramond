//! The scripted (WASM) AI node a namespaced (`mod_id:name`) brain-row `node` key resolves to.
//!
//! Block-behavior hooks run fire-and-forget after the world tick. This is different: the node's
//! decision feeds priority arbitration the same tick, so it runs before the brains decide. Mob
//! manager collects [`AiNodeRequest`] from every claimed [`ScriptedNode`], batches the whole
//! population through `modding::ai::dispatch_batch` (one guest call per node key), and replies come
//! back via `AiCtx::scripted` into [`WasmNodeAi::tick`]. Detached, decision-only, no sim scope (see
//! `GuestCall::AiNode`). Unclaimed key or disabled mod just means no request and no opinion, same
//! as an engine node returning defaults.
//!
//! Extra perception facts are pull-model: the brain row declares what its node reads (`"inputs":
//! ["player_held"]` in `mobs.json`, parsed into [`ScriptedInputs`]), and only declared facts get
//! computed. Adding one means a [`ScriptedInputs`] flag, a compute arm here, and an `AiNodeCtx`
//! field. Mobs that don't declare it never pay for it.

use crate::modding::ai::AiNodeRequest;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput, HeadLook};
use super::super::EntityRef;
pub use petramond_world::ai_vocab::ScriptedInputs;

use petramond_math::math::IVec3;

pub struct ScriptedNode {
    key: &'static str,
    inputs: ScriptedInputs,
}

impl ScriptedNode {
    fn claimed(&self) -> bool {
        crate::modding::ai::is_claimed(self.key)
    }

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
            player_foothold: (self.inputs.player_foothold
                && ctx.pos.distance_squared(ctx.player_pos)
                    <= f64::from(
                        crate::mob::PLAYER_REACTIVE_RANGE * crate::mob::PLAYER_REACTIVE_RANGE,
                    ))
            .then(|| super::chase::goal_cell_near(ctx, ctx.player_pos))
            .flatten()
            .map(|c| c.to_array()),
            tags: std::sync::Arc::clone(ctx.tags),
            tags_rev: ctx.tags_rev,
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
        if !self.node.claimed() {
            return BehaviorOutput::default();
        }
        let Some(d) = ctx.scripted.take_next() else {
            return BehaviorOutput::default();
        };
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
