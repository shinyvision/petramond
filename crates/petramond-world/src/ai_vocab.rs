//! Scripted-AI ADMISSION vocabulary: the engine AI-node name list, the
//! declarable scripted-input facts, and the raw `brain_extensions` shapes —
//! everything pack admission can check WITHOUT the loaded def table or the
//! node factories. The factories (and full def-aware validation) live in the
//! engine's `mob::behavior`; a test there pins its factory table to
//! [`ENGINE_AI_NODE_NAMES`] so the two cannot drift.

use serde::Deserialize;

/// Engine AI-node names, the admission-time half of `mob::behavior::node_spec`.
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

/// Whether `name` is a resolvable AI node at admission time: an engine node,
/// or a scripted (namespaced, non-engine) key.
pub fn node_known(name: &str) -> bool {
    ENGINE_AI_NODE_NAMES.contains(&name)
        || crate::registry::namespace(name)
            .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE)
}

/// The declarable scripted-node input facts a brain row may request. Engine
/// nodes read `AiCtx` directly and declare none (the loader rejects `inputs`
/// on them); only the scripted node ships facts across the ABI.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptedInputs {
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

/// The extension-only lenient view a pack's `mobs.json` gets at ADMISSION
/// (before any registry exists) — see [`validate_brain_extensions`].
#[derive(Deserialize)]
pub struct RawExtFile {
    #[serde(default)]
    pub brain_extensions: Vec<RawBrainExtension>,
}

/// One additive brain extension: nodes appended to `mob`'s merged row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBrainExtension {
    /// Registry name of the TARGET species — deliberately anyone's.
    pub mob: String,
    pub brain: Vec<RawBrainNode>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBrainNode {
    pub node: String,
    /// Omitted = the node's canonical slot (wander lowest, expression above it,
    /// chase above wander, attack on top — see `brain::PRIORITY_*`).
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub params: serde_json::Value,
    /// Scripted-node perception facts this node DECLARES it reads
    /// (`"inputs": ["player_held"]`) — only declared facts are computed and
    /// shipped per dispatch (see `behavior::ScriptedInputs`). Engine nodes
    /// read the sim context directly and must declare none.
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// Pack-admission validation of a layer's optional `brain_extensions`,
/// surfaced early (`manifest::registration_keys`) so a bad extension disables
/// the PACK — the admission contract — instead of panicking the whole catalog
/// load. Everything checkable WITHOUT the loaded def table is checked here:
/// the strict shape parse, node-key resolution through the same registry the
/// loader uses, and the declared-inputs vocabulary. Factory param validation
/// needs the target def and runs at catalog load, where a failing extension
/// is skipped with its source named (see the module docs).
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

/// The def-free half of extension-node validation (shared vocabulary with the
/// loader: `node_spec` + [`behavior::ScriptedInputs::parse`]).
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
