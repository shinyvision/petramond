//! The registry of animated player rigs, read from the layered
//! `animations/rigs.json` catalog. Each row is a rig: its model, its animator
//! document, whether other players OBSERVE it, and the PRESENTER that draws
//! it — with the rig conventions that presenter reads (where each hand grips,
//! the bone the view rides, the torso bones a head-look untwists, the clip a
//! hand rests in per held render kind). Nothing in code names a rig's bones
//! or hold clips; a pack re-points any of them through its own copy of the
//! catalog.
//!
//! The engine draws two presenters — the body every observer sees, and the
//! first-person viewmodel — so the catalog holds one rig per presenter: a
//! row for a presenter another row already has is refused, because nothing
//! would draw it. A new kind of rig is a new presenter.
//!
//! The mod ABI names a rig; the name resolves to a [`RigId`] once at the
//! host call, and everything below — the claims, the wire rows, the join
//! tables, the render drivers — carries the id.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use petramond_world::animation::{ClipId, Graph};
use petramond_world::bbmodel::Model;
use petramond_world::item::ItemRenderKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use mod_api::rig::{PLAYER_BODY, PLAYER_FIRST_PERSON};

/// The catalog's pack-relative path.
const CATALOG_PATH: &str = "animations/rigs.json";

const ROW_KEYS: &[&str] = &[
    "model", "animator", "observed", "presenter", "grips", "camera", "twist", "holds",
];

/// A rig's index in this process's registry — the compact id every runtime
/// path carries below the ABI. Both mirrors resolve names against the same
/// registry; a peer whose table differs remaps by name at the transport.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RigId(pub u16);

impl RigId {
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// What draws a rig.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presenter {
    /// Posed per player over its locomotion and drawn in the world.
    Body,
    /// Posed for the local player and drawn in the hand pass, its camera
    /// bone moving the view.
    Viewmodel,
}

impl Presenter {
    fn named(name: &str) -> Option<Self> {
        match name {
            "body" => Some(Self::Body),
            "viewmodel" => Some(Self::Viewmodel),
            _ => None,
        }
    }
}

/// One registered rig.
pub struct Rig {
    pub name: String,
    pub model: Model,
    /// The animator document's pack-relative path.
    pub animator: String,
    /// `None` when the rig's animator document is missing or its own layer
    /// is refused (logged once at load): the rig then draws its rest pose
    /// and answers no animator claim.
    pub graph: Option<Arc<Graph>>,
    /// Whether OTHER players see this rig. Only an observed rig's claims and
    /// fired events are replicated to observers; a player's own state keeps
    /// every rig.
    pub observed: bool,
    pub presenter: Presenter,
    /// The bones each hand grips at, `[main, off]`.
    pub grips: [usize; 2],
    /// The bone the rendered view rides, for a presenter that draws a view.
    pub camera: Option<usize>,
    /// The torso bones whose yaw a head-look takes back out.
    pub twist: Vec<usize>,
    /// The clip a hand rests in while it holds each render kind
    /// ([`ItemRenderKind::name`]); a kind not listed rests in the rig's rest.
    pub holds: Vec<(&'static str, ClipId)>,
}

impl Rig {
    /// The clip a hand holding `kind` rests in, if the row names one.
    pub fn hold(&self, kind: ItemRenderKind) -> Option<ClipId> {
        self.holds
            .iter()
            .find(|(name, _)| *name == kind.name())
            .map(|(_, clip)| *clip)
    }
}

static RIGS: LazyLock<Vec<Rig>> = LazyLock::new(|| {
    let mut doc = Map::new();
    for layer in petramond_world::assets::read_catalog_layers(CATALOG_PATH) {
        match serde_json::from_str::<Value>(&layer.text) {
            Ok(value) => {
                if let Err(e) = petramond_world::assets::merge_object(&mut doc, Some(&value), "rigs") {
                    log::error!("rigs catalog {}: {e}", layer.path.display());
                }
            }
            Err(e) => log::error!("rigs catalog {}: json: {e}", layer.path.display()),
        }
    }
    let (rigs, errors) = rigs_from(&doc, load_model, super::animator::load_animator);
    for e in errors {
        log::error!("rigs catalog: {e}");
    }
    rigs
});

fn load_model(path: &str) -> Option<Model> {
    let Some((src, _)) = petramond_world::assets::read_bytes(path) else {
        log::error!("rig model '{path}' not found in the asset roots");
        return None;
    };
    let key = Path::new(path).file_stem()?.to_str()?;
    petramond_world::asset_cache::load_or_compile::<Model>(key, &src)
        .map_err(|e| log::error!("rig model '{path}' precache failed: {e}"))
        .ok()
}

/// The rigs a merged catalog describes, rows in name order, with one error
/// per row refused. A row is a rig only whole: its model loads, every bone
/// and hold clip it names exists, and its presenter is not already taken.
pub(super) fn rigs_from(
    doc: &Map<String, Value>,
    load_model: impl Fn(&str) -> Option<Model>,
    load_graph: impl Fn(&str, &Model) -> Option<Arc<Graph>>,
) -> (Vec<Rig>, Vec<String>) {
    let mut rigs: Vec<Rig> = Vec::new();
    let mut errors = Vec::new();
    let mut names: Vec<&String> = doc.keys().collect();
    names.sort();
    for name in names {
        match rig_from(name, &doc[name], &load_model, &load_graph) {
            Ok(rig) if rigs.iter().any(|r| r.presenter == rig.presenter) => errors.push(format!(
                "{name}: its presenter already draws another rig; nothing would draw this one"
            )),
            Ok(rig) => rigs.push(rig),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    (rigs, errors)
}

fn rig_from(
    name: &str,
    row: &Value,
    load_model: &impl Fn(&str) -> Option<Model>,
    load_graph: &impl Fn(&str, &Model) -> Option<Arc<Graph>>,
) -> Result<Rig, String> {
    let row = row.as_object().ok_or("a rig row is an object")?;
    if let Some(key) = row.keys().find(|k| !ROW_KEYS.contains(&k.as_str())) {
        return Err(format!("unknown key `{key}`"));
    }
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("`{key}` names a path"))
    };
    let model_path = text("model")?;
    let animator = text("animator")?.to_string();
    let observed = row
        .get("observed")
        .and_then(Value::as_bool)
        .ok_or("`observed` is true or false")?;
    let presenter = row
        .get("presenter")
        .and_then(Value::as_str)
        .and_then(Presenter::named)
        .ok_or("`presenter` is `body` or `viewmodel`")?;
    let model = load_model(model_path).ok_or_else(|| format!("no model at '{model_path}'"))?;
    let bone = |bone: &Value, key: &str| {
        let name = bone
            .as_str()
            .ok_or_else(|| format!("`{key}` names bones"))?;
        model
            .bone_named(name)
            .ok_or_else(|| format!("`{key}`: the model has no bone `{name}`"))
    };
    let grips = row.get("grips").and_then(Value::as_object).ok_or("`grips` is { main, off }")?;
    let grip = |hand: &str| {
        grips
            .get(hand)
            .ok_or_else(|| format!("`grips.{hand}` names a bone"))
            .and_then(|v| bone(v, "grips"))
    };
    let grips = [grip("main")?, grip("off")?];
    let camera = row.get("camera").map(|v| bone(v, "camera")).transpose()?;
    let twist = match row.get("twist") {
        None => Vec::new(),
        Some(Value::Array(list)) => list.iter().map(|v| bone(v, "twist")).collect::<Result<_, _>>()?,
        Some(_) => return Err("`twist` is a list of bones".into()),
    };
    let graph = load_graph(&animator, &model);
    let mut holds = Vec::new();
    if let Some(listed) = row.get("holds") {
        let listed = listed.as_object().ok_or("`holds` is { render kind: clip }")?;
        for (kind, clip) in listed {
            let kind = ItemRenderKind::NAMES
                .into_iter()
                .find(|k| *k == kind.as_str())
                .ok_or_else(|| format!("`holds.{kind}`: not a render kind ({})", ItemRenderKind::NAMES.join(", ")))?;
            let clip = clip
                .as_str()
                .ok_or_else(|| format!("`holds.{kind}` names a clip"))?;
            // Without a graph the rig draws its rest pose, so a hold has
            // nothing to rest in; the missing animator is already logged.
            if let Some(graph) = &graph {
                let id = graph
                    .clips()
                    .id(clip)
                    .ok_or_else(|| format!("`holds.{kind}`: the rig has no clip `{clip}`"))?;
                holds.push((kind, id));
            }
        }
    }
    Ok(Rig {
        name: name.to_string(),
        model,
        animator,
        graph,
        observed,
        presenter,
        grips,
        camera,
        twist,
        holds,
    })
}

/// Every registered rig, in id order.
pub fn all() -> &'static [Rig] {
    &RIGS
}

/// The rig behind `id`; `None` for an id this process never registered (a
/// remapped peer row can carry none — the transport drops those).
pub fn get(id: RigId) -> Option<&'static Rig> {
    RIGS.get(id.index())
}

/// Resolve a rig NAME to its id, once at the ABI.
pub fn id(name: &str) -> Option<RigId> {
    RIGS.iter()
        .position(|r| r.name == name)
        .map(|i| RigId(i as u16))
}

/// The rig `presenter` draws, with its id.
pub fn presented(presenter: Presenter) -> Option<(RigId, &'static Rig)> {
    RIGS.iter()
        .position(|r| r.presenter == presenter)
        .map(|i| (RigId(i as u16), &RIGS[i]))
}

/// The compiled animator of `id`'s rig, if it has one.
pub fn graph(id: RigId) -> Option<&'static Arc<Graph>> {
    get(id)?.graph.as_ref()
}

/// Whether other players observe `id`'s rig (an unregistered id is not
/// observed by anyone).
pub fn observed(id: RigId) -> bool {
    get(id).is_some_and(|r| r.observed)
}

#[cfg(test)]
mod tests;
