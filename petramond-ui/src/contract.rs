mod fit;

pub use fit::{overflows, viewport_overflow, Overflow, SMALLEST_VIEWPORT};

use crate::doc::DocClass::{Container, Hud, Screen};
use crate::doc::{Accept, DocClass, Document, Node, NodeKind};
use crate::validate::{DocIssue, SlotContract, StyleLookup};

pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const ENGINE_NAMESPACE: &str = "petramond";

pub const MAIN_GRID_SLOTS: usize = 27;
pub const HOTBAR_SLOTS: usize = 9;
pub const CHEST_SLOTS: usize = 27;
pub const FURNACE_SLOTS: usize = 3;
pub const MAX_CONTAINER_SLOTS: usize = 54;
pub const MAX_SLOT_FILTERS: usize = u32::BITS as usize;
pub const IMAGE_MAX_SIDE: u32 = 640;
pub const IMAGE_MAX_FRAMES: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineKind {
    pub key: &'static str,
    pub class: DocClass,
    pub slots: &'static [(&'static str, usize)],
}

impl EngineKind {
    pub fn contract(&self) -> SlotContract {
        SlotContract::new(self.slots)
    }
}

const fn kind(
    key: &'static str,
    class: DocClass,
    slots: &'static [(&'static str, usize)],
) -> EngineKind {
    EngineKind { key, class, slots }
}

pub const ENGINE_KINDS: &[EngineKind] = &[
    kind(
        "petramond:chest",
        Container,
        &[
            ("container", CHEST_SLOTS),
            ("player_inv", MAIN_GRID_SLOTS),
            ("hotbar", HOTBAR_SLOTS),
        ],
    ),
    kind(
        "petramond:inventory",
        Container,
        &[
            ("player_inv", MAIN_GRID_SLOTS),
            ("hotbar", HOTBAR_SLOTS),
            ("off_hand", 1),
            ("craft_result", 1),
        ],
    ),
    kind(
        "petramond:crafting_table",
        Container,
        &[
            ("player_inv", MAIN_GRID_SLOTS),
            ("hotbar", HOTBAR_SLOTS),
            ("craft_result", 1),
        ],
    ),
    kind(
        "petramond:furnace",
        Container,
        &[
            ("player_inv", MAIN_GRID_SLOTS),
            ("hotbar", HOTBAR_SLOTS),
            ("container", FURNACE_SLOTS),
        ],
    ),
    kind(
        "petramond:hotbar",
        Hud,
        &[("hotbar", HOTBAR_SLOTS), ("off_hand", 1)],
    ),
    kind("petramond:furniture_workbench", Container, &[]),
    kind("petramond:title", Screen, &[]),
    kind("petramond:world_select", Screen, &[]),
    kind("petramond:world_settings", Screen, &[]),
    kind("petramond:create_world", Screen, &[]),
    kind("petramond:delete_world", Screen, &[]),
    kind("petramond:pause", Screen, &[]),
    kind("petramond:demo", Screen, &[("demo_slots", 9)]),
    kind("petramond:sleep", Screen, &[]),
    kind("petramond:death", Screen, &[]),
    kind("petramond:connect_server", Screen, &[]),
    kind("petramond:mods_missing", Screen, &[]),
    kind("petramond:connection_lost", Screen, &[]),
    kind("petramond:options", Screen, &[]),
    kind("petramond:options_sound", Screen, &[]),
    kind("petramond:options_controls", Screen, &[]),
    kind("petramond:options_graphics", Screen, &[]),
    kind("petramond:creative", Container, &[("hotbar", HOTBAR_SLOTS)]),
    kind("petramond:schematics", Screen, &[]),
    kind("petramond:chiseling_station", Container, &[]),
    kind("petramond:account", Screen, &[]),
    kind("petramond:account_sign_in", Screen, &[]),
    kind("petramond:content", Screen, &[]),
];

pub fn engine_kind(key: &str) -> Option<&'static EngineKind> {
    ENGINE_KINDS.iter().find(|k| k.key == key)
}

pub fn namespace(key: &str) -> Option<&str> {
    match key.split_once(':') {
        Some((ns, name)) if !ns.is_empty() && !name.is_empty() => Some(ns),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindClass {
    Engine(&'static EngineKind),
    Mod,
}

pub fn classify_kind(key: &str) -> Result<KindClass, String> {
    if let Some(kind) = engine_kind(key) {
        return Ok(KindClass::Engine(kind));
    }
    match namespace(key) {
        Some(ENGINE_NAMESPACE) => Err(format!("unknown kind '{key}'")),
        Some(_) => Ok(KindClass::Mod),
        None => Err(format!(
            "unknown kind '{key}' (mod kinds must be namespaced 'mod_id:name')"
        )),
    }
}

pub fn kind_permitted(key: &str, pack_id: Option<&str>) -> Result<(), String> {
    if !matches!(classify_kind(key), Ok(KindClass::Mod)) {
        return Ok(());
    }
    let owner = namespace(key).unwrap_or("");
    match pack_id {
        Some(id) if id == owner => Ok(()),
        Some(id) => Err(format!(
            "kind '{key}' does not belong to pack '{id}' (namespaced kinds must use the \
             shipping pack's own id)"
        )),
        None => Err(format!(
            "kind '{key}' is namespaced but the document ships outside any pack"
        )),
    }
}

pub fn mod_document_contract(doc: &Document) -> Result<SlotContract, String> {
    let mut roles: Vec<(String, usize)> = Vec::new();
    for (role, count) in doc.role_slots() {
        match role.as_str() {
            "container" if count <= MAX_CONTAINER_SLOTS => {}
            "container" => {
                return Err(format!(
                    "declares {count} container slots; the cap is {MAX_CONTAINER_SLOTS}"
                ))
            }
            "player_inv" if count == MAIN_GRID_SLOTS => {}
            "hotbar" if count == HOTBAR_SLOTS => {}
            "off_hand" if count == 1 => {}
            "player_inv" | "hotbar" | "off_hand" => {
                return Err(format!(
                    "role '{role}' declares {count} slots; the engine grids are \
                     player_inv:{MAIN_GRID_SLOTS}, hotbar:{HOTBAR_SLOTS}, and off_hand:1"
                ))
            }
            _ => {
                return Err(format!(
                    "role '{role}' is not available to mod documents (allowed: container, \
                     player_inv, hotbar, off_hand)"
                ))
            }
        }
        roles.push((role, count));
    }
    Ok(SlotContract { roles })
}

pub fn contract_for_document(doc: &Document) -> Result<SlotContract, String> {
    match classify_kind(&doc.kind)? {
        KindClass::Engine(kind) => Ok(kind.contract()),
        KindClass::Mod => mod_document_contract(doc),
    }
}

pub trait EngineCatalog {
    fn item_tag_exists(&self, name: &str) -> bool;
}

pub struct EngineCheck<'a> {
    pub styles: Option<&'a dyn StyleLookup>,
    pub pack_id: Option<&'a str>,
    pub catalog: &'a dyn EngineCatalog,
    pub image_size: &'a dyn Fn(&str) -> Option<(u32, u32)>,
}

pub fn validate_for_engine(doc: &Document, check: &EngineCheck<'_>) -> Vec<DocIssue> {
    let document_issue = |message: String| DocIssue {
        path: "document".into(),
        message,
    };
    let mut issues = Vec::new();
    if let Err(e) = kind_permitted(&doc.kind, check.pack_id) {
        issues.push(document_issue(e));
    }
    let contract = match contract_for_document(doc) {
        Ok(contract) => Some(contract),
        Err(e) => {
            issues.push(document_issue(e));
            None
        }
    };
    issues.extend(doc.validate(check.styles, contract.as_ref()));
    issues.extend(
        slot_semantics_issues(doc, check.catalog)
            .into_iter()
            .map(document_issue),
    );
    issues.extend(image_issues(doc, check.image_size));
    issues
}

pub fn slot_semantics_issues(doc: &Document, catalog: &dyn EngineCatalog) -> Vec<String> {
    let mut issues = Vec::new();
    for cell in doc.slot_semantics() {
        if cell.role != "container" {
            if !cell.accepts.is_empty() || cell.take_only {
                issues.push(format!(
                    "role '{}' carries accepts/take_only; slot semantics apply only to \
                     'container' slots",
                    cell.role
                ));
            }
            continue;
        }
        if cell.accepts.len() > MAX_SLOT_FILTERS {
            issues.push(format!(
                "a container slot declares {} accepts filters; the cap is {MAX_SLOT_FILTERS} \
                 (the runtime accepts mask spends one bit per filter)",
                cell.accepts.len()
            ));
        }
        for accept in &cell.accepts {
            match accept {
                Accept::Tag(name) if !catalog.item_tag_exists(name) => {
                    issues.push(format!("unknown item tag '{name}' in a slot's accepts"));
                }
                Accept::Data { data } if namespace(data).is_none() => issues.push(format!(
                    "slot accepts data key '{data}': data keys must be namespaced ('mod_id:name')"
                )),
                _ => {}
            }
        }
    }
    issues
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub name: String,
    pub frames: Option<[u32; 2]>,
    pub path: String,
}

pub fn image_refs(doc: &Document) -> Vec<ImageRef> {
    fn walk(node: &Node, path: &str, refs: &mut Vec<ImageRef>) {
        let named = match &node.kind {
            NodeKind::Image { image, frames, .. } => Some((image.as_str(), *frames)),
            NodeKind::Rotimage { image, .. } => Some((image.as_str(), None)),
            NodeKind::Button {
                image: Some(image),
                frames,
                ..
            } => Some((image.as_str(), *frames)),
            NodeKind::Button {
                icon: Some(icon), ..
            }
            | NodeKind::Toggle { icon: Some(icon) }
                if crate::is_doc_image_icon(icon) =>
            {
                Some((icon.as_str(), None))
            }
            _ => None,
        };
        if let Some((name, frames)) = named.filter(|(name, _)| !name.is_empty()) {
            match refs.iter_mut().find(|r| r.name == name) {
                Some(seen) => {
                    if seen.frames.is_none() && frames.is_some() {
                        seen.frames = frames;
                        seen.path = node_label(path, node);
                    }
                }
                None => refs.push(ImageRef {
                    name: name.to_owned(),
                    frames,
                    path: node_label(path, node),
                }),
            }
        }
        for (i, child) in node.children.iter().enumerate() {
            walk(child, &format!("{path}/{i}"), refs);
        }
    }
    let mut refs = Vec::new();
    walk(&doc.root, "root", &mut refs);
    refs
}

pub(crate) fn node_label(path: &str, node: &Node) -> String {
    match &node.id {
        Some(id) => format!("{path}({}#{id})", node.kind.type_name()),
        None => format!("{path}({})", node.kind.type_name()),
    }
}

pub fn image_issues(
    doc: &Document,
    image_size: &dyn Fn(&str) -> Option<(u32, u32)>,
) -> Vec<DocIssue> {
    let mut issues = Vec::new();
    for r in image_refs(doc) {
        let message = match image_size(&r.name) {
            None => format!("names missing art {}", r.name),
            Some(size) => match r.frames.map(|f| check_frame_sheet(&r.name, size, f)) {
                Some(Err(e)) => e,
                _ => continue,
            },
        };
        issues.push(DocIssue {
            path: r.path,
            message,
        });
    }
    issues
}

pub fn check_frame_sheet(name: &str, size: (u32, u32), frames: [u32; 2]) -> Result<(), String> {
    let [cols, rows] = frames;
    let (w, h) = size;
    if cols == 0 || rows == 0 || w % cols != 0 || h % rows != 0 {
        return Err(format!(
            "image {name} is {w}x{h}, which the frames grid {cols}x{rows} does not divide evenly"
        ));
    }
    if cols * rows > IMAGE_MAX_FRAMES {
        return Err(format!(
            "image {name} declares {} frames; the cap is {IMAGE_MAX_FRAMES}",
            cols * rows
        ));
    }
    if w > IMAGE_MAX_SIDE || h > IMAGE_MAX_SIDE {
        return Err(format!(
            "image {name} is {w}x{h}; the GUI image side cap is {IMAGE_MAX_SIDE}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
