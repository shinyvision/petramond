//! The engine's document contract: which kinds exist, which slot roles each
//! one pins, and every load-time rule the game applies before it trusts a
//! document to route clicks.
//!
//! This is the one place in the crate that knows about engine kinds. It is
//! pure data plus pure checks — registry lookups the game owns (item tags)
//! come in through [`EngineCatalog`], image files through a size callback —
//! so the game's loader and the gui-builder run literally the same rules,
//! and a document the builder calls valid is one the game will load.
//!
//! The numeric limits below are the GUI-facing statement of engine
//! constants; the game asserts at compile time that its own constants agree,
//! so neither side can drift silently.

mod fit;

pub use fit::{viewport_overflow, SMALLEST_VIEWPORT};

use crate::doc::DocClass::{Container, Hud, Screen};
use crate::doc::{Accept, DocClass, Document, Node, NodeKind};
use crate::validate::{DocIssue, SlotContract, StyleLookup};

/// The crate version the rules come from, for "valid for the engine at …"
/// labels in tools.
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The namespace engine kinds live in; a mod may never claim it.
pub const ENGINE_NAMESPACE: &str = "petramond";

/// The player's main inventory grid (everything but the hotbar).
pub const MAIN_GRID_SLOTS: usize = 27;
/// The hotbar row.
pub const HOTBAR_SLOTS: usize = 9;
/// A chest's storage.
pub const CHEST_SLOTS: usize = 27;
/// A furnace's input, fuel and output cells.
pub const FURNACE_SLOTS: usize = 3;
/// Cap on a mod document's `container` role slots (engine-backed storage).
pub const MAX_CONTAINER_SLOTS: usize = 54;
/// Cap on one slot's `accepts` filters: the runtime mask spends one bit each.
pub const MAX_SLOT_FILTERS: usize = u32::BITS as usize;
/// Largest side of a framed GUI sheet (the client image side cap).
pub const IMAGE_MAX_SIDE: u32 = 640;
/// Most frames one GUI sheet may declare.
pub const IMAGE_MAX_FRAMES: u32 = 64;

/// One engine document kind: its key, the class its document is authored
/// as, and the slot roles (with exact counts) its screen routes.
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

/// Every engine kind, in the game's frozen kind-id order (the game's tests
/// pin the order against its registry). Append-only.
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
    // Input, fuel, output — the in-role index IS the container index.
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
    kind(
        "petramond:creative",
        Container,
        &[("hotbar", HOTBAR_SLOTS)],
    ),
    kind("petramond:schematics", Screen, &[]),
    kind("petramond:chiseling_station", Container, &[]),
];

/// The engine kind named `key`, if it is one.
pub fn engine_kind(key: &str) -> Option<&'static EngineKind> {
    ENGINE_KINDS.iter().find(|k| k.key == key)
}

/// The namespace of `key` (`"wheel:wheel"` → `Some("wheel")`), or `None`
/// for bare and degenerate forms.
pub fn namespace(key: &str) -> Option<&str> {
    match key.split_once(':') {
        Some((ns, name)) if !ns.is_empty() && !name.is_empty() => Some(ns),
        _ => None,
    }
}

/// How the engine classifies a document's kind key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindClass {
    /// An engine kind with its fixed contract.
    Engine(&'static EngineKind),
    /// A pack-registered `mod_id:name` kind.
    Mod,
}

/// Classify `key` the way the game's kind registry does: engine keys map to
/// their table row; a namespaced key outside the engine namespace is a mod
/// kind; anything else (a bare name, an unknown `petramond:*`) is refused.
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

/// The mod-kind ownership rule: a namespaced document kind must ship from
/// the pack owning the namespace. Engine kinds may ship from anywhere
/// (re-skin packs).
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

/// The slot contract a MOD document earns from its own declarations: mod
/// kinds may declare generic `container` slots (engine-backed mod-owned
/// storage, capped), plus the standard `player_inv`/`hotbar`/`off_hand`
/// grids with the engine counts. Any other role is refused — a mod document
/// can never name an engine block-entity's roles.
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

/// The contract a document is validated against: its engine kind's
/// fixed table row, or the contract a mod document earns from its own slots.
pub fn contract_for_document(doc: &Document) -> Result<SlotContract, String> {
    match classify_kind(&doc.kind)? {
        KindClass::Engine(kind) => Ok(kind.contract()),
        KindClass::Mod => mod_document_contract(doc),
    }
}

/// The registry lookups load-time validation needs from the host.
pub trait EngineCatalog {
    /// Whether an item tag named `name` is registered — a QUERY, never an
    /// interning resolve (which would register a typo as a fresh empty tag
    /// and make the check unreachable).
    fn item_tag_exists(&self, name: &str) -> bool;
}

/// Everything [`validate_for_engine`] checks a document against.
pub struct EngineCheck<'a> {
    /// The theme's style set (`None` skips style checks, e.g. while art is
    /// work in progress).
    pub styles: Option<&'a dyn StyleLookup>,
    /// The id of the pack the document ships in (`None`: base assets or no
    /// pack at all).
    pub pack_id: Option<&'a str>,
    pub catalog: &'a dyn EngineCatalog,
    /// Pixel size of a statically named image resolved beside the document,
    /// or `None` when the file is missing.
    pub image_size: &'a dyn Fn(&str) -> Option<(u32, u32)>,
}

/// Every reason the game would refuse `doc` at load (empty = it loads):
/// kind classification and namespace ownership, the per-kind slot contract,
/// structural and style rules, slot `accepts` semantics, and the art the
/// document names.
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
    issues.extend(slot_semantics_issues(doc, check.catalog).into_iter().map(document_issue));
    issues.extend(image_issues(doc, check.image_size));
    issues
}

/// The slot `accepts`/`take_only` rules: semantics apply only to
/// `container` slots, one slot carries at most [`MAX_SLOT_FILTERS`]
/// filters, a TAG must be registered, and a DATA key must be namespaced.
/// Data keys have no declaration anywhere (a row states one by carrying
/// it), so "no row carries it yet" is a pack shipping its slot before its
/// rows, not an error.
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

/// One image a document statically names, with the first frames grid seen
/// for it and the node an issue about it is anchored to (the first
/// reference, or the node whose grid was kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub name: String,
    pub frames: Option<[u32; 2]>,
    /// Node path in the validator's `root/2/0(image)` form.
    pub path: String,
}

/// Every image a document statically names — on `image`, `rotimage`, and
/// image-backed `button` nodes alike — in first-reference order (the order
/// that feeds `TexId::DocImage`). The first SEEN frames grid is kept, so an
/// unframed reference cannot hide a framed one from the sheet check. An
/// empty name is the runtime-bound pattern (`bind.image` supplies the art)
/// and names no file.
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

/// A node's issue path: its structural path plus `(type#id)`, the form
/// [`Document::validate`] anchors issues with.
pub(crate) fn node_label(path: &str, node: &Node) -> String {
    match &node.id {
        Some(id) => format!("{path}({}#{id})", node.kind.type_name()),
        None => format!("{path}({})", node.kind.type_name()),
    }
}

/// Art the document names that the game would refuse: a missing file, or a
/// framed sheet whose grid does not fit (see [`check_frame_sheet`]).
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

/// A framed sheet is uploaded whole and ONE frame is drawn per node, so the
/// grid must divide the image exactly and both must stay inside the shared
/// GUI image bounds — a bad grid mis-slices every frame of the sheet.
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
