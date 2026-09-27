use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use petramond_anim::{ClipId, Graph};
use petramond_world::bbmodel::Model;
use petramond_world::item::ItemRenderKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use mod_api::rig::{PLAYER_BODY, PLAYER_FIRST_PERSON};

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

#[derive(
    Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
pub struct RigId(pub u16);

impl RigId {
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presenter {
    Body,
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

pub struct Rig {
    pub name: String,
    pub model: Model,
    pub animator: String,
    pub graph: Option<Arc<Graph>>,
    pub observed: bool,
    pub presenter: Presenter,
    pub grips: [usize; 2],
    pub camera: Option<usize>,
    pub twist: Vec<usize>,
    pub holds: Vec<(&'static str, ClipId)>,
}

impl Rig {
    pub fn hold(&self, kind: ItemRenderKind) -> Option<ClipId> {
        self.holds
            .iter()
            .find(|(name, _)| *name == kind.name())
            .map(|(_, clip)| *clip)
    }
}

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

pub fn all() -> &'static [Rig] {
    rigs()
}

pub fn get(id: RigId) -> Option<&'static Rig> {
    rigs().get(id.index())
}

pub fn id(name: &str) -> Option<RigId> {
    rigs()
        .iter()
        .position(|r| r.name == name)
        .map(|i| RigId(i as u16))
}

pub fn presented(presenter: Presenter) -> Option<(RigId, &'static Rig)> {
    let rigs = rigs();
    rigs.iter()
        .position(|r| r.presenter == presenter)
        .map(|i| (RigId(i as u16), &rigs[i]))
}

pub fn graph(id: RigId) -> Option<&'static Arc<Graph>> {
    get(id)?.graph.as_ref()
}

pub fn observed(id: RigId) -> bool {
    get(id).is_some_and(|r| r.observed)
}

#[cfg(test)]
mod tests;
