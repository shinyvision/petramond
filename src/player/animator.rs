//! A rig's animator document — the graph over the rig's own clips plus any
//! Bedrock clip libraries — LAYERED like every catalog: the base document,
//! then each pack's copy in load order, merged by key so a pack adds a rule,
//! a gate, a layer, a param or a library without restating the engine's.
//! Shared by every animated player rig, and the ABI's name→id resolution for
//! the animator claims.
//!
//! One document is flat JSON: `libraries` (Bedrock `.animation.json` paths,
//! each clip under the shipping pack's namespace), and the graph's own
//! `params`, `events`, `slots`, `masks`, `markers`, `gates`, `layers` and
//! `rules`. Layers merge by `name`, gates and rules by `id`: a row naming an
//! existing one replaces it in place, `"enabled": false` removes it, a new
//! row appends or goes `"before"` a named one — the locomotion table's
//! overlay rule, applied to the graph.
//!
//! A layer that does not compile is LEFT OUT, named in the log, and the rig
//! loads the rest: one pack's typo never takes a rig's animation away from
//! every other pack.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mod_api::AnimationClipInfo;
use petramond_world::animation::{ClipLibrary, Graph};
use petramond_world::assets::{self, CatalogLayer};
use petramond_world::bbmodel::{bedrock, Model};
use serde_json::{Map, Value};

use super::rigs::{self, Rig, RigId};
use super::{AnimatorParam, AnimatorPlay};

/// The namespace the engine's own clips, and any library from an id-less
/// pack, are filed under.
const ENGINE_NAMESPACE: &str = "petramond";

const DOC_KEYS: &[&str] = &[
    "libraries", "params", "events", "slots", "masks", "markers", "gates", "layers", "rules",
];

/// A rig graph's declared vocabulary in id order — what a session hands a
/// peer so the ids on its rows remap by name. `rig` is the rig's registry
/// name, so a peer matches tables by name rather than by position.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AnimatorNames {
    pub rig: String,
    pub clips: Vec<String>,
    pub params: Vec<String>,
    pub slots: Vec<String>,
    pub events: Vec<String>,
}

impl AnimatorNames {
    pub fn of(rig: &Rig) -> Self {
        let Some(graph) = &rig.graph else {
            return Self {
                rig: rig.name.clone(),
                ..Self::default()
            };
        };
        Self {
            rig: rig.name.clone(),
            clips: graph.clips().names().to_vec(),
            params: graph.param_names().map(str::to_string).collect(),
            slots: graph.slot_names().to_vec(),
            events: graph.event_names().to_vec(),
        }
    }

    /// Every registered rig's table, in rig-id order.
    pub fn all() -> Vec<Self> {
        rigs::all().iter().map(Self::of).collect()
    }
}

/// A rig by its ABI name, with its graph; `Err` names what is missing.
fn rig_graph(name: &str) -> Result<(RigId, &'static Arc<Graph>), String> {
    let id = rigs::id(name).ok_or_else(|| format!("no rig named `{name}`"))?;
    let graph = rigs::graph(id).ok_or_else(|| format!("the `{name}` rig has no animator"))?;
    Ok((id, graph))
}

fn small(index: usize, what: &str) -> Result<u16, String> {
    u16::try_from(index).map_err(|_| format!("too many {what}"))
}

/// Resolve a caller's params to rig and graph ids, once at the ABI; `Err`
/// names the rig or param the engine lacks.
pub fn resolve_params(params: Vec<mod_api::AnimatorParam>) -> Result<Vec<AnimatorParam>, String> {
    params
        .into_iter()
        .map(|p| {
            let (rig, graph) = rig_graph(&p.rig)?;
            let id = graph
                .param(&p.param)
                .ok_or_else(|| format!("the `{}` rig's graph has no param `{}`", p.rig, p.param))?;
            Ok(AnimatorParam {
                rig,
                param: small(id.index(), "params")?,
                value: p.value,
            })
        })
        .collect()
}

/// Resolve a caller's plays to rig, graph and library ids, once at the ABI;
/// `Err` names the rig, slot or clip the engine lacks.
pub fn resolve_plays(plays: Vec<mod_api::AnimatorPlay>) -> Result<Vec<AnimatorPlay>, String> {
    plays
        .into_iter()
        .map(|p| {
            let (rig, graph) = rig_graph(&p.rig)?;
            let slot = graph
                .slot(&p.slot)
                .ok_or_else(|| format!("the `{}` rig's graph has no slot `{}`", p.rig, p.slot))?;
            let clip = graph
                .clips()
                .id(&p.clip)
                .ok_or_else(|| format!("the `{}` rig has no clip `{}`", p.rig, p.clip))?;
            Ok(AnimatorPlay {
                rig,
                slot: small(slot.index(), "slots")?,
                clip: small(clip.index(), "clips")?,
                clock: p.clock,
                mirror: p.mirror,
                priority: p.priority,
            })
        })
        .collect()
}

/// Resolve an event name on one rig's graph, once at the ABI.
pub fn resolve_event(rig: &str, event: &str) -> Result<(RigId, u16), String> {
    let (id, graph) = rig_graph(rig)?;
    let event_id = graph
        .event(event)
        .ok_or_else(|| format!("the `{rig}` rig's graph declares no event `{event}`"))?;
    Ok((id, small(event_id.index(), "events")?))
}

/// One clip's length, loop and markers in time order; `None` when the rig
/// has no such clip.
pub fn clip_info(rig: &str, name: &str) -> Option<AnimationClipInfo> {
    let clips = rig_graph(rig).ok()?.1.clips();
    let clip = clips.get(clips.id(name)?);
    let mut markers: Vec<(String, f32)> =
        clip.markers().iter().map(|m| (m.name.clone(), m.time)).collect();
    markers.sort_by(|a, b| a.1.total_cmp(&b.1));
    Some(AnimationClipInfo {
        length: clip.length,
        looping: clip.looping,
        markers,
    })
}

/// One document as it merges: the text, the namespace its libraries' clips
/// take, and where it came from — its directory resolves its library paths,
/// so two packs shipping the same relative path never shadow each other.
pub struct AnimatorSource {
    pub text: String,
    pub namespace: String,
    /// Where it came from, for errors.
    pub origin: String,
    /// The directory its `libraries` paths are relative to.
    pub dir: PathBuf,
    /// The rig's own document: its libraries may restate the rig's clips.
    /// Any other layer's clip must be new, since a silent replacement is
    /// how one pack quietly rewrites another's motion.
    pub engine: bool,
}

/// Merge `layers` (lowest priority first) and compile the result against
/// `rig`, refusing the whole on any error. `read` fetches a library by the
/// path its layer's directory resolves.
pub fn compile_animator<'a>(
    layers: impl IntoIterator<Item = &'a AnimatorSource>,
    rig: &Model,
    read: impl Fn(&Path) -> Option<Vec<u8>>,
) -> Result<Graph, String> {
    let layers: Vec<&AnimatorSource> = layers.into_iter().collect();
    let mut libraries: Vec<(String, &AnimatorSource)> = Vec::new();
    let mut params = Map::new();
    let mut events: Vec<Value> = Vec::new();
    let mut slots: Vec<Value> = Vec::new();
    let mut masks = Map::new();
    let mut markers = Map::new();
    let mut gates: Vec<Value> = Vec::new();
    let mut graph_layers: Vec<Value> = Vec::new();
    let mut rules: Vec<Value> = Vec::new();
    for &layer in &layers {
        let at = |e: String| format!("{}: {e}", layer.origin);
        let doc: Value = serde_json::from_str(&layer.text).map_err(|e| at(format!("json: {e}")))?;
        let doc = doc
            .as_object()
            .ok_or_else(|| at("an animator document is a JSON object".into()))?;
        if let Some(key) = doc.keys().find(|k| !DOC_KEYS.contains(&k.as_str())) {
            return Err(at(format!("unknown key `{key}`")));
        }
        match doc.get("libraries") {
            None => {}
            Some(Value::Array(paths)) => {
                for path in paths {
                    let path = path
                        .as_str()
                        .ok_or_else(|| at("libraries: expected a list of paths".into()))?;
                    libraries.push((path.to_string(), layer));
                }
            }
            Some(_) => return Err(at("libraries: expected a list".into())),
        }
        assets::merge_object(&mut params, doc.get("params"), "params").map_err(at)?;
        assets::union(&mut events, doc.get("events"), "events").map_err(at)?;
        assets::union(&mut slots, doc.get("slots"), "slots").map_err(at)?;
        assets::merge_object(&mut masks, doc.get("masks"), "masks").map_err(at)?;
        assets::merge_object(&mut markers, doc.get("markers"), "markers").map_err(at)?;
        assets::merge_rows(&mut gates, doc.get("gates"), "gates", "id").map_err(at)?;
        assets::merge_rows(&mut graph_layers, doc.get("layers"), "layers", "name").map_err(at)?;
        assets::merge_rows(&mut rules, doc.get("rules"), "rules", "id").map_err(at)?;
    }

    let mut clips = ClipLibrary::from_model(rig, ENGINE_NAMESPACE);
    for (path, layer) in libraries {
        let at = |e: String| format!("{}: library '{path}': {e}", layer.origin);
        let bytes = read(&layer.dir.join(&path)).ok_or_else(|| at("not found".into()))?;
        let text = std::str::from_utf8(&bytes).map_err(|e| at(e.to_string()))?;
        if layer.engine {
            clips.add_bedrock(text, rig, &layer.namespace).map_err(at)?;
            continue;
        }
        let parsed = bedrock::parse_library(text, |name| rig.bone_named(name)).map_err(at)?;
        for (name, clip) in parsed {
            let full = format!("{}:{name}", layer.namespace);
            if clips.id(&full).is_some() {
                return Err(at(format!(
                    "clip `{full}` already exists on the rig; a layer adds clips, it does not replace them"
                )));
            }
            clips.insert(&full, clip);
        }
    }
    let mut merged = Map::new();
    merged.insert("params".into(), Value::Object(params));
    merged.insert("events".into(), Value::Array(events));
    merged.insert("slots".into(), Value::Array(slots));
    merged.insert("masks".into(), Value::Object(masks));
    merged.insert("markers".into(), Value::Object(markers));
    merged.insert("gates".into(), Value::Array(gates));
    merged.insert("layers".into(), Value::Array(graph_layers));
    merged.insert("rules".into(), Value::Array(rules));
    Graph::compile(&Value::Object(merged).to_string(), rig, clips).map_err(|e| {
        let origins: Vec<&str> = layers.iter().map(|l| l.origin.as_str()).collect();
        format!("{}: {e}", origins.join(" + "))
    })
}

/// Merge and compile every layer that can join: the rig's own document
/// first, then each pack's layer in order, kept only while the document
/// still compiles with it. Answers the graph and, beside it, one error per
/// layer left out. `None` when the first document does not compile on its
/// own.
pub fn compile_layers(
    layers: &[AnimatorSource],
    rig: &Model,
    read: impl Fn(&Path) -> Option<Vec<u8>>,
) -> (Option<Graph>, Vec<String>) {
    let mut accepted: Vec<&AnimatorSource> = Vec::new();
    let mut graph = None;
    let mut refused = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        let candidate = accepted.iter().copied().chain(std::iter::once(layer));
        match compile_animator(candidate, rig, &read) {
            Ok(compiled) => {
                graph = Some(compiled);
                accepted.push(layer);
            }
            Err(e) if i == 0 => return (None, vec![e]),
            Err(e) => refused.push(format!("left out {}: {e}", layer.origin)),
        }
    }
    (graph, refused)
}

/// Every layer of the animator document at `path`, lowest priority first,
/// each carrying its pack's namespace for the libraries it adds.
pub fn animator_layers(path: &str) -> Vec<AnimatorSource> {
    let packs = assets::layers();
    assets::read_catalog_layers(path)
        .into_iter()
        .map(|CatalogLayer { text, path, owner, .. }| AnimatorSource {
            text,
            // An id-less pack files its clips under the engine's namespace
            // but is still a pack: only the copy outside every pack directory
            // is the rig's own document.
            engine: owner.is_none() && !packs.iter().any(|l| path.starts_with(&l.dir)),
            namespace: owner.unwrap_or_else(|| ENGINE_NAMESPACE.to_string()),
            origin: path.display().to_string(),
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
        })
        .collect()
}

/// Load, merge and compile the animator document at `path` against `rig`,
/// logging every layer left out; `None` (logged) when the document is
/// missing or its first layer does not compile.
pub(super) fn load_animator(path: &str, rig: &Model) -> Option<Arc<Graph>> {
    let layers = animator_layers(path);
    if layers.is_empty() {
        log::error!("animator '{path}': not found in the asset roots");
        return None;
    }
    let (graph, errors) = compile_layers(&layers, rig, |library| std::fs::read(library).ok());
    for e in &errors {
        match graph {
            Some(_) => log::error!("animator '{path}': {e}"),
            None => log::error!("animator '{path}': the rig has no animator: {e}"),
        }
    }
    graph.map(Arc::new)
}

#[cfg(test)]
mod tests;
