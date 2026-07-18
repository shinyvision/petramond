//! Mob AI behaviors — one composable unit each, à la the block behaviors — plus the
//! string-keyed AI-NODE REGISTRY that `mobs.json` brain rows resolve through.
//!
//! A species' brain is a data list `[{node, priority, params}]` on its `mobs.json`
//! row; [`factory`] maps each `node` key to the engine constructor that builds the
//! behavior from its (load-validated) `params` + the owning [`MobDef`] row. Adding an
//! engine behavior is: add a file, add its `mod` + `pub use`, add its key here — no
//! change to the brain's arbitration or the navigator. A NAMESPACED
//! (`mod_id:key`) node key resolves to the scripted [`wasm::WasmNodeAi`],
//! which forwards each decision to the mod that claimed the key via
//! `RegisterAiNode` (see that module).

mod chase;
mod contact;
mod head_look;
mod hearing;
mod idle_anim;
mod los;
mod melee;
mod retaliate;
#[cfg(test)]
pub(crate) mod test_support;
mod wander;
mod wasm;

pub use chase::ChasePlayerAi;
pub use contact::ChaseContactAi;
pub use head_look::HeadLookAi;
pub use hearing::ChaseSoundAi;
pub use idle_anim::IdleAnimAi;
pub use melee::MeleeAttackAi;
pub use retaliate::RetaliateAi;
pub use wander::WanderAi;
pub(crate) use wasm::ScriptedInputs;

use super::brain::{
    AiBehavior, PRIORITY_ATTACK, PRIORITY_CHASE, PRIORITY_CONTACT, PRIORITY_EXPRESSION,
    PRIORITY_RETALIATE, PRIORITY_WANDER,
};
use super::load::NodeFactory;
use super::MobDef;

/// One engine AI node's registry entry: its factory plus the canonical priority
/// slot a brain row gets when it doesn't state one.
pub(super) struct NodeSpec {
    pub factory: NodeFactory,
    pub default_priority: u8,
}

/// Resolve an AI-node key to its registry entry, or `None` for a key the
/// engine doesn't implement (the loader turns that into a load error). A
/// namespaced non-engine key resolves to the scripted WASM node; brains
/// should state its `priority` explicitly (the default slots it with wander).
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
        "chase_sound" => NodeSpec {
            factory: chase_sound_node,
            default_priority: PRIORITY_CHASE,
        },
        "chase_contact" => NodeSpec {
            factory: chase_contact_node,
            default_priority: PRIORITY_CONTACT,
        },
        "retaliate" => NodeSpec {
            factory: retaliate_node,
            default_priority: PRIORITY_RETALIATE,
        },
        "melee_attack" => NodeSpec {
            factory: melee_attack_node,
            default_priority: PRIORITY_ATTACK,
        },
        // The reserved engine namespace never dispatches to a mod.
        _ if crate::registry::namespace(name)
            .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE) =>
        {
            NodeSpec {
                factory: wasm_node,
                default_priority: PRIORITY_WANDER,
            }
        }
        _ => return None,
    })
}

/// Idle roaming, tuned entirely by the owning row's `wander` / `habitat` /
/// `avoid_water` fields (they stay row data because spawn and habitat code read
/// them too) — the node itself takes no params.
fn wander_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    no_inputs(inputs)?;
    Ok(Box::new(WanderAi::new(
        def.wander,
        &def.habitat,
        def.avoid_water,
    )))
}

fn head_look_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    no_inputs(inputs)?;
    Ok(Box::new(HeadLookAi::new()))
}

fn idle_anim_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    no_inputs(inputs)?;
    Ok(Box::new(IdleAnimAi::new()))
}

fn chase_player_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_inputs(inputs)?;
    Ok(Box::new(ChasePlayerAi::from_params(params)?))
}

fn chase_sound_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_inputs(inputs)?;
    Ok(Box::new(ChaseSoundAi::from_params(params, all)?))
}

fn chase_contact_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_inputs(inputs)?;
    Ok(Box::new(ChaseContactAi::from_params(params)?))
}

fn retaliate_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_inputs(inputs)?;
    Ok(Box::new(RetaliateAi::from_params(params)?))
}

fn melee_attack_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_inputs(inputs)?;
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

/// Reject a declared `inputs` list on an engine node — engine behaviors read
/// `AiCtx` directly; declared inputs exist to bound what crosses the ABI to a
/// scripted node, and accepting them here would silently do nothing.
fn no_inputs(inputs: ScriptedInputs) -> Result<(), String> {
    if inputs.is_empty() {
        Ok(())
    } else {
        Err("'inputs' are only declarable on scripted (mod_id:name) nodes".into())
    }
}

/// Scripted WASM node: routes each decision on its row key. Takes no params
/// (a mod configures itself from its own pack data); the row's declared
/// `inputs` select which perception facts are computed and shipped per
/// dispatch.
fn wasm_node(
    node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_params(params)?;
    Ok(Box::new(wasm::WasmNodeAi::new(node, inputs)))
}
