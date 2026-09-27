use std::path::{Path, PathBuf};
use std::sync::Arc;

use mod_api::AnimationClipInfo;
use petramond_anim::{ClipLibrary, Graph};
use petramond_world::assets::{self, CatalogLayer};
use petramond_world::bbmodel::{bedrock, Model};
use serde_json::{Map, Value};

use super::rigs::{self, Rig, RigId};
use super::{AnimatorParam, AnimatorPlay};

const ENGINE_NAMESPACE: &str = "petramond";

const DOC_KEYS: &[&str] = &[
    "libraries",
    "params",
    "events",
    "slots",
    "masks",
    "markers",
    "gates",
    "layers",
    "rules",
];

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

    pub fn all() -> Vec<Self> {
        rigs::all().iter().map(Self::of).collect()
    }
}

fn rig_graph(name: &str) -> Result<(RigId, &'static Arc<Graph>), String> {
    let id = rigs::id(name).ok_or_else(|| format!("no rig named `{name}`"))?;
    let graph = rigs::graph(id).ok_or_else(|| format!("the `{name}` rig has no animator"))?;
    Ok((id, graph))
}

fn small(index: usize, what: &str) -> Result<u16, String> {
    u16::try_from(index).map_err(|_| format!("too many {what}"))
}

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

pub fn resolve_event(rig: &str, event: &str) -> Result<(RigId, u16), String> {
    let (id, graph) = rig_graph(rig)?;
    let event_id = graph
        .event(event)
        .ok_or_else(|| format!("the `{rig}` rig's graph declares no event `{event}`"))?;
    Ok((id, small(event_id.index(), "events")?))
}

pub fn clip_info(rig: &str, name: &str) -> Option<AnimationClipInfo> {
    let clips = rig_graph(rig).ok()?.1.clips();
    let clip = clips.get(clips.id(name)?);
    let mut markers: Vec<(String, f32)> = clip
        .markers()
        .iter()
        .map(|m| (m.name.clone(), m.time))
        .collect();
    markers.sort_by(|a, b| a.1.total_cmp(&b.1));
    Some(AnimationClipInfo {
        length: clip.length,
        looping: clip.looping,
        markers,
    })
}

pub struct AnimatorSource {
    pub text: String,
    pub namespace: String,
    pub origin: String,
    pub dir: PathBuf,
    pub engine: bool,
}

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

pub fn animator_layers(path: &str) -> Vec<AnimatorSource> {
    let packs = assets::layers();
    assets::read_catalog_layers(path)
        .into_iter()
        .map(
            |CatalogLayer {
                 text, path, owner, ..
             }| AnimatorSource {
                text,
                engine: owner.is_none() && !packs.iter().any(|l| path.starts_with(&l.dir)),
                namespace: owner.unwrap_or_else(|| ENGINE_NAMESPACE.to_string()),
                origin: path.display().to_string(),
                dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            },
        )
        .collect()
}

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
