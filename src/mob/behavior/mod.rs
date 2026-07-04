//! Mob AI behaviors — one composable unit each, à la the block behaviors — plus the
//! string-keyed AI-NODE REGISTRY that `mobs.json` brain rows resolve through.
//!
//! A species' brain is a data list `[{node, priority, params}]` on its `mobs.json`
//! row; [`factory`] maps each `node` key to the engine constructor that builds the
//! behavior from its (load-validated) `params` + the owning [`MobDef`] row. Adding an
//! engine behavior is: add a file, add its `mod` + `pub use`, add its key here — no
//! change to the brain's arbitration or the navigator. Namespaced (`mod_id:key`) node
//! keys are RESERVED: scripted (WASM) AI nodes are future work, so a row naming one
//! refuses to load rather than silently doing nothing.

mod chase;
mod head_look;
mod idle_anim;
mod melee;
mod wander;

pub use chase::ChasePlayerAi;
pub use head_look::HeadLookAi;
pub use idle_anim::IdleAnimAi;
pub use melee::MeleeAttackAi;
pub use wander::WanderAi;

use super::brain::{
    AiBehavior, PRIORITY_ATTACK, PRIORITY_CHASE, PRIORITY_EXPRESSION, PRIORITY_WANDER,
};
use super::load::NodeFactory;
use super::MobDef;

/// One engine AI node's registry entry: its factory plus the canonical priority
/// slot a brain row gets when it doesn't state one.
pub(super) struct NodeSpec {
    pub factory: NodeFactory,
    pub default_priority: u8,
}

/// Resolve an AI-node key to its engine registry entry, or `None` for a key the
/// engine doesn't implement (the loader turns that into a load error,
/// distinguishing unknown bare keys from reserved namespaced ones).
pub(super) fn node_spec(name: &str) -> Option<NodeSpec> {
    Some(match name {
        "wander" => NodeSpec {
            factory: wander_node,
            default_priority: PRIORITY_WANDER,
        },
        "head_look" => NodeSpec {
            factory: head_look_node,
            default_priority: PRIORITY_EXPRESSION,
        },
        "idle_anim" => NodeSpec {
            factory: idle_anim_node,
            default_priority: PRIORITY_EXPRESSION,
        },
        "chase_player" => NodeSpec {
            factory: chase_player_node,
            default_priority: PRIORITY_CHASE,
        },
        "melee_attack" => NodeSpec {
            factory: melee_attack_node,
            default_priority: PRIORITY_ATTACK,
        },
        _ => return None,
    })
}

/// Idle roaming, tuned entirely by the owning row's `wander` / `habitat` /
/// `avoid_water` fields (they stay row data because spawn and habitat code read
/// them too) — the node itself takes no params.
fn wander_node(
    params: &serde_json::Value,
    def: &'static MobDef,
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    Ok(Box::new(WanderAi::new(
        def.wander,
        &def.habitat,
        def.avoid_water,
    )))
}

fn head_look_node(
    params: &serde_json::Value,
    _def: &'static MobDef,
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    Ok(Box::new(HeadLookAi::new()))
}

fn idle_anim_node(
    params: &serde_json::Value,
    _def: &'static MobDef,
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    Ok(Box::new(IdleAnimAi::new()))
}

fn chase_player_node(
    params: &serde_json::Value,
    _def: &'static MobDef,
) -> Result<Box<dyn AiBehavior>, String> {
    Ok(Box::new(ChasePlayerAi::from_params(params)?))
}

fn melee_attack_node(
    params: &serde_json::Value,
    _def: &'static MobDef,
) -> Result<Box<dyn AiBehavior>, String> {
    Ok(Box::new(MeleeAttackAi::from_params(params)?))
}

/// Reject params on a node that takes none, so a typo'd tuning key fails the load
/// instead of being silently ignored.
fn no_params(params: &serde_json::Value) -> Result<(), String> {
    match params {
        serde_json::Value::Null => Ok(()),
        serde_json::Value::Object(m) if m.is_empty() => Ok(()),
        _ => Err("this node takes no params".into()),
    }
}
