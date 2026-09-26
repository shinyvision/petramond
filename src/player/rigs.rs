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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use petramond_anim::{ClipId, Graph};
use petramond_world::bbmodel::Model;
use petramond_world::item::ItemRenderKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use mod_api::rig::{PLAYER_BODY, PLAYER_FIRST_PERSON};

/// The catalog's pack-relative path.
const CATALOG_PATH: &str = "animations/rigs.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRig {
    model: String,
    animator: String,
    observed: bool,
    presenter: String,
    grips: RawGrips,
    #[serde(default)]
    camera: Option<String>,
    #[serde(default)]
    twist: Vec<String>,
    #[serde(default)]
    holds: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGrips {
    main: String,
    off: String,
}

/// A rig's index in this process's registry — the compact id every runtime
/// path carries below the ABI. Both mirrors resolve names against the same
/// registry; a peer whose table differs remaps by name at the transport.
#[derive(
    Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
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

/// Every rig of a content registry — a derived view built on first use. Rig
/// rows are presentation: a bad row is logged and skipped rather than failing
/// the registry.
static RIGS: petramond_world::content::Slot<Vec<Rig>> =
    petramond_world::content::Slot::new("player rigs", &[], load_rigs);

fn load_rigs(reg: &petramond_world::content::ContentRegistry) -> Result<Vec<Rig>, String> {
    let mut doc = Map::new();
    for layer in reg.packs().read_catalog_layers(CATALOG_PATH) {
        match serde_json::from_str::<Value>(&layer.text) {
            Ok(value) => {
                if let Err(e) =
                    petramond_world::assets::merge_object(&mut doc, Some(&value), "rigs")
                {
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
    Ok(rigs)
}

fn rigs() -> &'static [Rig] {
    RIGS.current()
}

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
    let row: RawRig = serde_json::from_value(row.clone()).map_err(|e| e.to_string())?;
    let presenter =
        Presenter::named(&row.presenter).ok_or("`presenter` is `body` or `viewmodel`")?;
    let model = load_model(&row.model).ok_or_else(|| format!("no model at '{}'", row.model))?;
    let bone = |name: &str, key: &str| {
        model
            .bone_named(name)
            .ok_or_else(|| format!("`{key}`: the model has no bone `{name}`"))
    };
    let grips = [
        bone(&row.grips.main, "grips.main")?,
        bone(&row.grips.off, "grips.off")?,
    ];
    let camera = row
        .camera
        .as_deref()
        .map(|v| bone(v, "camera"))
        .transpose()?;
    let twist = row
        .twist
        .iter()
        .map(|v| bone(v, "twist"))
        .collect::<Result<_, _>>()?;
    let graph = load_graph(&row.animator, &model);
    let mut holds = Vec::new();
    for (kind, clip) in &row.holds {
        let kind = ItemRenderKind::NAMES
            .into_iter()
            .find(|k| *k == kind.as_str())
            .ok_or_else(|| {
                format!(
                    "`holds.{kind}`: not a render kind ({})",
                    ItemRenderKind::NAMES.join(", ")
                )
            })?;
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
    Ok(Rig {
        name: name.to_string(),
        model,
        animator: row.animator,
        graph,
        observed: row.observed,
        presenter,
        grips,
        camera,
        twist,
        holds,
    })
}

/// Every registered rig, in id order.
pub fn all() -> &'static [Rig] {
    rigs()
}

/// The rig behind `id`; `None` for an id this process never registered (a
/// remapped peer row can carry none — the transport drops those).
pub fn get(id: RigId) -> Option<&'static Rig> {
    rigs().get(id.index())
}

/// Resolve a rig NAME to its id, once at the ABI.
pub fn id(name: &str) -> Option<RigId> {
    rigs()
        .iter()
        .position(|r| r.name == name)
        .map(|i| RigId(i as u16))
}

/// The rig `presenter` draws, with its id.
pub fn presented(presenter: Presenter) -> Option<(RigId, &'static Rig)> {
    let rigs = rigs();
    rigs.iter()
        .position(|r| r.presenter == presenter)
        .map(|i| (RigId(i as u16), &rigs[i]))
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
