mod chase;
mod contact;
mod escape;
mod head_look;
mod hearing;
mod idle_anim;
pub(crate) mod los;
mod melee;
mod panic;
mod retaliate;
#[cfg(test)]
pub mod test_support;
mod wander;
mod wasm;

pub use chase::ChasePlayerAi;
pub use contact::ChaseContactAi;
pub use head_look::HeadLookAi;
pub use hearing::ChaseSoundAi;
pub use idle_anim::IdleAnimAi;
pub use melee::MeleeAttackAi;
pub use panic::PanicAi;
pub use retaliate::RetaliateAi;
pub use wander::WanderAi;
pub use wasm::{ScriptedInputs, ScriptedNode};

use super::brain::{
    AiBehavior, PRIORITY_ATTACK, PRIORITY_CHASE, PRIORITY_CONTACT, PRIORITY_DAMAGE_RESPONSE,
    PRIORITY_EXPRESSION, PRIORITY_WANDER,
};
use super::load::NodeFactory;
use super::MobDef;

pub(super) struct NodeSpec {
    pub factory: NodeFactory,
    pub default_priority: u8,
}

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
        "panic" => NodeSpec {
            factory: panic_node,
            default_priority: PRIORITY_DAMAGE_RESPONSE,
        },
        "retaliate" => NodeSpec {
            factory: retaliate_node,
            default_priority: PRIORITY_DAMAGE_RESPONSE,
        },
        "melee_attack" => NodeSpec {
            factory: melee_attack_node,
            default_priority: PRIORITY_ATTACK,
        },
        _ if petramond_world::registry::namespace(name)
            .is_some_and(|ns| ns != petramond_world::registry::ENGINE_NAMESPACE) =>
        {
            NodeSpec {
                factory: wasm_node,
                default_priority: PRIORITY_WANDER,
            }
        }
        _ => return None,
    })
}

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
        def.avoid_fluids,
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

fn panic_node(
    _node: &'static str,
    params: &serde_json::Value,
    inputs: ScriptedInputs,
    _def: &'static MobDef,
    _all: &[MobDef],
) -> Result<Box<dyn AiBehavior>, String> {
    no_inputs(inputs)?;
    Ok(Box::new(PanicAi::from_params(params)?))
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

fn no_params(params: &serde_json::Value) -> Result<(), String> {
    match params {
        serde_json::Value::Null => Ok(()),
        serde_json::Value::Object(m) if m.is_empty() => Ok(()),
        _ => Err("this node takes no params".into()),
    }
}

fn no_inputs(inputs: ScriptedInputs) -> Result<(), String> {
    if inputs.is_empty() {
        Ok(())
    } else {
        Err("'inputs' are only declarable on scripted (mod_id:name) nodes".into())
    }
}

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
