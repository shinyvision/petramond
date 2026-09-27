use serde::Deserialize;

pub const ENGINE_AI_NODE_NAMES: &[&str] = &[
    "wander",
    "head_look",
    "idle_anim",
    "chase_player",
    "chase_sound",
    "chase_contact",
    "retaliate",
    "melee_attack",
];

pub fn node_known(name: &str) -> bool {
    ENGINE_AI_NODE_NAMES.contains(&name)
        || crate::registry::namespace(name)
            .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE)
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptedInputs {
    pub player_held: bool,
    pub player_foothold: bool,
}

impl ScriptedInputs {
    const KNOWN: &'static [&'static str] = &["player_held", "player_foothold"];

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

#[derive(Deserialize)]
pub struct RawExtFile {
    #[serde(default)]
    pub brain_extensions: Vec<RawBrainExtension>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBrainExtension {
    pub mob: String,
    pub brain: Vec<RawBrainNode>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBrainNode {
    pub node: String,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub inputs: Vec<String>,
}

pub fn validate_brain_extensions(text: &str) -> Result<(), String> {
    let file = serde_json::from_str::<RawExtFile>(text)
        .map_err(|e| format!("invalid brain_extensions: {e}"))?;
    for ext in &file.brain_extensions {
        for node in &ext.brain {
            admission_check_node(node).map_err(|e| {
                format!(
                    "invalid brain_extensions: extension for '{}': node '{}': {e}",
                    ext.mob, node.node
                )
            })?;
        }
    }
    Ok(())
}

fn admission_check_node(node: &RawBrainNode) -> Result<(), String> {
    if !node_known(&node.node) {
        return Err(format!("unknown AI node '{}'", node.node));
    }
    let inputs = ScriptedInputs::parse(&node.inputs)?;
    let scripted = crate::registry::namespace(&node.node)
        .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE);
    if !scripted && !inputs.is_empty() {
        return Err("'inputs' are only declarable on scripted (mod_id:name) nodes".into());
    }
    Ok(())
}
