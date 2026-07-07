//! The GUI-document registry: runtime-loaded `*.gui.json` documents from
//! `assets/ui/documents/`, pack-overlayable by file name.
//!
//! Load rules: a namespaced document kind must ship from the pack that owns
//! the namespace; engine kinds may ship from anywhere (re-skin packs).
//! Documents validate against the engine's per-kind
//! [`SlotContract`] and the theme's style set — a bad document is skipped
//! loudly, never trusted to route clicks.
//!
//! In debug builds the registry re-reads changed files (~1s poll), so editing
//! a document (or re-exporting from the gui-builder) shows up without a
//! restart.

use super::GuiKind;
use crate::container::{SlotSpec, MAX_CONTAINER_SLOTS};
use llama_ui::{Document, SlotContract};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Where GUI documents live, relative to the asset roots.
const DOCUMENTS_DIR: &str = "ui/documents";

pub(crate) struct DocEntry {
    pub kind: GuiKind,
    pub doc: Arc<Document>,
    /// Every image the document references (resolved beside the document, in
    /// first-reference order): the index is the `TexId::DocImage` id both
    /// layout (natural sizes) and the renderer (bind groups) use.
    pub images: Arc<Vec<DocImageRef>>,
    /// A mod document's `container` role slot semantics, in-role index order
    /// (empty for engine kinds and widgets-only mod GUIs). Resolved once at
    /// load from the slot nodes' `accepts`/`take_only` fields.
    pub container_slots: Arc<Vec<SlotSpec>>,
}

#[derive(Clone, Debug)]
pub(crate) struct DocImageRef {
    pub name: String,
    pub path: PathBuf,
    pub size: (u32, u32),
}

/// A cheap handle to one loaded document.
#[derive(Clone)]
pub(crate) struct DocRef {
    pub doc: Arc<Document>,
    pub images: Arc<Vec<DocImageRef>>,
    pub container_slots: Arc<Vec<SlotSpec>>,
}

struct Registry {
    entries: Vec<DocEntry>,
    /// Every file that fed the registry, with its mtime (debug reload).
    sources: Vec<(PathBuf, Option<SystemTime>)>,
    last_check: Instant,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// The document for `kind`, if one is loaded.
pub(crate) fn doc_for(kind: GuiKind) -> Option<DocRef> {
    let mut guard = REGISTRY.lock().unwrap();
    let registry = guard.get_or_insert_with(load);
    if cfg!(debug_assertions) && registry.last_check.elapsed() > Duration::from_secs(1) {
        let changed = registry
            .sources
            .iter()
            .any(|(path, mtime)| file_mtime(path) != *mtime);
        if changed {
            *registry = load();
        } else {
            registry.last_check = Instant::now();
        }
    }
    registry
        .entries
        .iter()
        .find(|e| e.kind == kind)
        .map(|e| DocRef {
            doc: e.doc.clone(),
            images: e.images.clone(),
            container_slots: e.container_slots.clone(),
        })
}

/// A mod document's `container` role slot semantics, in-role index order.
/// Empty for engine kinds, widgets-only mod GUIs, and unknown kinds.
pub(crate) fn container_slot_specs(kind: GuiKind) -> Arc<Vec<SlotSpec>> {
    doc_for(kind)
        .map(|d| d.container_slots)
        .unwrap_or_default()
}

/// The slot contract a MOD document earns from its own declarations: mod
/// kinds may declare generic `container` slots (engine-backed mod-owned
/// storage, capped), plus the standard `player_inv`/`hotbar` grids with the
/// engine counts. Any other role is refused — a mod document can never name
/// an engine block-entity's roles. `Err` skips the document loudly at load.
fn mod_contract_for(doc: &Document) -> Result<SlotContract, String> {
    // The engine grids' sizes come from the inventory layout itself.
    const MAIN_GRID: usize = crate::inventory::TOTAL_SLOTS - crate::inventory::HOTBAR_LEN;
    const HOTBAR: usize = crate::inventory::HOTBAR_LEN;
    let mut roles: Vec<(String, usize)> = Vec::new();
    for (role, count) in doc.role_slots() {
        match role.as_str() {
            "container" if count <= MAX_CONTAINER_SLOTS => {}
            "container" => {
                return Err(format!(
                    "declares {count} container slots; the cap is {MAX_CONTAINER_SLOTS}"
                ))
            }
            "player_inv" if count == MAIN_GRID => {}
            "hotbar" if count == HOTBAR => {}
            "player_inv" | "hotbar" => {
                return Err(format!(
                    "role '{role}' declares {count} slots; the engine grids are \
                     player_inv:{MAIN_GRID} and hotbar:{HOTBAR}"
                ))
            }
            _ => {
                return Err(format!(
                    "role '{role}' is not available to mod documents (allowed: container, \
                     player_inv, hotbar)"
                ))
            }
        }
        roles.push((role, count));
    }
    Ok(SlotContract { roles })
}

/// A mod document's `container` slot semantics in in-role index order,
/// resolving the slot nodes' `accepts` tag names against the item-tag
/// registry. Unknown tag names are a document error (`Err` skips it loudly).
fn doc_container_specs(doc: &Document) -> Result<Vec<SlotSpec>, String> {
    let mut specs = Vec::new();
    for cell in doc.slot_semantics() {
        if cell.role != "container" {
            if !cell.accepts.is_empty() || cell.take_only {
                return Err(format!(
                    "role '{}' carries accepts/take_only; slot semantics apply only to \
                     'container' slots",
                    cell.role
                ));
            }
            continue;
        }
        let mut tags = Vec::new();
        for name in &cell.accepts {
            match crate::item::ItemTag::from_key(name) {
                Some(tag) => tags.push(tag),
                None => return Err(format!("unknown item tag '{name}' in a slot's accepts")),
            }
        }
        specs.push(SlotSpec {
            accepts: tags,
            take_only: cell.take_only,
        });
    }
    Ok(specs)
}

/// The engine's slot expectations per kind. Mod kinds derive their contract
/// from their own document via [`mod_contract_for`]; shell kinds carry no
/// role slots.
pub(crate) fn contract_for(kind: GuiKind) -> SlotContract {
    match kind {
        GuiKind::Chest => SlotContract::new(&[("storage", 27), ("player_inv", 27), ("hotbar", 9)]),
        GuiKind::Inventory => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("craft_input", 4),
            ("craft_result", 1),
        ]),
        GuiKind::CraftingTable => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("craft_input", 9),
            ("craft_result", 1),
        ]),
        GuiKind::Furnace => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("furnace_input", 1),
            ("furnace_fuel", 1),
            ("furnace_output", 1),
        ]),
        GuiKind::Hotbar => SlotContract::new(&[("hotbar", 9)]),
        GuiKind::FurnitureWorkbench => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("workbench_input", 1),
            ("workbench_result", 21),
        ]),
        GuiKind::Demo => SlotContract::new(&[("demo_slots", 9)]),
        _ => SlotContract::default(),
    }
}

/// The mod-kind ownership rule, shared with the baked path: a namespaced
/// document kind must ship from the pack owning the namespace.
fn kind_permitted(kind: GuiKind, pack_id: Option<&str>) -> Result<(), String> {
    if !kind.is_mod() {
        return Ok(());
    }
    let key = super::kind_key(kind).unwrap_or("?");
    let owner = key.split_once(':').map(|(ns, _)| ns).unwrap_or("");
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

fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn load() -> Registry {
    struct Found {
        json: PathBuf,
        dir: PathBuf,
        pack_id: Option<String>,
    }
    // Overlay by file name: base roots first, packs after — the last copy of
    // a name wins, so packs shadow base documents.
    let mut manifests: Vec<(String, Found)> = Vec::new();
    let mut sources: Vec<(PathBuf, Option<SystemTime>)> = Vec::new();
    for (dir, pack_id) in crate::assets::layer_dirs_with_ids(DOCUMENTS_DIR) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".gui.json") {
                continue;
            }
            let found = Found {
                json: path,
                dir: dir.clone(),
                pack_id: pack_id.clone(),
            };
            match manifests.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = found,
                None => manifests.push((name, found)),
            }
        }
    }
    manifests.sort_by(|a, b| a.0.cmp(&b.0));

    let theme = super::doc_theme::theme();
    let mut entries = Vec::new();
    for (_, found) in manifests {
        sources.push((found.json.clone(), file_mtime(&found.json)));
        let Ok(text) = std::fs::read_to_string(&found.json) else {
            continue;
        };
        let doc = match Document::from_json(&text) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("gui: ignoring {} — {e}", found.json.display());
                continue;
            }
        };
        let Some(kind) = super::intern_kind(&doc.kind) else {
            eprintln!(
                "gui: ignoring {} — unknown kind '{}'",
                found.json.display(),
                doc.kind
            );
            continue;
        };
        if let Err(e) = kind_permitted(kind, found.pack_id.as_deref()) {
            eprintln!("gui: ignoring {} — {e}", found.json.display());
            continue;
        }
        let contract = if kind.is_mod() {
            match mod_contract_for(&doc) {
                Ok(contract) => contract,
                Err(e) => {
                    eprintln!("gui: ignoring {} — {e}", found.json.display());
                    continue;
                }
            }
        } else {
            contract_for(kind)
        };
        let issues = doc.validate(Some(theme.as_ref()), Some(&contract));
        if !issues.is_empty() {
            for issue in &issues {
                eprintln!("gui: {} — {issue}", found.json.display());
            }
            continue;
        }
        let container_slots = match doc_container_specs(&doc) {
            Ok(specs) => specs,
            Err(e) => {
                eprintln!("gui: ignoring {} — {e}", found.json.display());
                continue;
            }
        };
        // Collect referenced images (resolved beside the document) with
        // their pixel sizes for layout naturals.
        let mut images: Vec<DocImageRef> = Vec::new();
        doc.root.visit(&mut |node| {
            let name = match &node.kind {
                llama_ui::NodeKind::Image { image, .. } => image,
                llama_ui::NodeKind::Rotimage { image, .. } => image,
                _ => return,
            };
            // An empty static name is the runtime-bound pattern (`bind.image`
            // supplies the art, e.g. the world-settings pack icons) — there is
            // no file to resolve beside the document.
            if name.is_empty() || images.iter().any(|i| &i.name == name) {
                return;
            }
            let path = found.dir.join(name);
            match image::image_dimensions(&path) {
                Ok(size) => images.push(DocImageRef {
                    name: name.clone(),
                    path,
                    size,
                }),
                Err(_) => eprintln!(
                    "gui: {} names missing art {name}; the quad will not draw",
                    found.json.display()
                ),
            }
        });
        entries.push(DocEntry {
            kind,
            doc: Arc::new(doc),
            images: Arc::new(images),
            container_slots: Arc::new(container_slots),
        });
    }
    Registry {
        entries,
        sources,
        last_check: Instant::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_contracts_cover_every_container_kind() {
        // The contract is the load-time guard that keeps a bad document from
        // mis-routing clicks; every slot-bearing kind must pin its counts.
        for (kind, total) in [
            (GuiKind::Chest, 27 + 27 + 9),
            (GuiKind::Inventory, 27 + 9 + 4 + 1),
            (GuiKind::CraftingTable, 27 + 9 + 9 + 1),
            (GuiKind::Furnace, 27 + 9 + 3),
            (GuiKind::Hotbar, 9),
            (GuiKind::FurnitureWorkbench, 27 + 9 + 1 + 21),
        ] {
            let contract = contract_for(kind);
            let sum: usize = contract.roles.iter().map(|(_, n)| n).sum();
            assert_eq!(sum, total, "{kind:?}");
        }
        assert!(contract_for(GuiKind::Pause).roles.is_empty());
    }

    #[test]
    fn bindings_catalog_parses_and_covers_controller_kinds() {
        // assets/ui/bindings.json is the builder-facing data contract; keep
        // it shipping and covering every screen a controller populates.
        let (text, _) =
            crate::assets::read_base_text("ui/bindings.json").expect("bindings catalog ships");
        let v: serde_json::Value = serde_json::from_str(&text).expect("catalog is valid JSON");
        let kinds = v["kinds"].as_object().expect("catalog has kinds");
        for key in [
            "llama:title",
            "llama:world_select",
            "llama:world_settings",
            "llama:create_world",
            "llama:delete_world",
            "llama:pause",
            "llama:sleep",
            "llama:death",
            "llama:hotbar",
            "llama:furnace",
        ] {
            assert!(
                kinds.contains_key(key),
                "{key} missing from bindings catalog"
            );
        }
    }

    #[test]
    fn overlay_documents_ship_and_validate() {
        // The sleep and death overlays are engine screens: a document that
        // fails validation is skipped loudly at load and the screen would
        // draw (and route) nothing — pin that the shipped ones load.
        for kind in [GuiKind::Sleep, GuiKind::Death] {
            assert!(doc_for(kind).is_some(), "{kind:?} document loads");
        }
    }

    #[test]
    fn mod_pack_documents_register_and_resolve_their_images() {
        // The wheel pack ships ui/documents/wheel.gui.json with a rotimage +
        // pointer image beside it; loading must register the namespaced kind
        // and resolve both images (they feed TexId::DocImage by index).
        let kind = crate::gui::intern_kind("wheel:wheel").expect("namespaced kind interns");
        let doc = doc_for(kind).expect("wheel pack document loads");
        assert_eq!(doc.doc.kind, "wheel:wheel");
        assert_eq!(doc.images.len(), 2, "face + pointer resolve beside the doc");
    }

    #[test]
    fn foreign_namespace_documents_are_rejected_per_pack() {
        let kind = crate::gui::intern_kind("doctest:owned").unwrap();
        assert!(kind_permitted(kind, Some("doctest")).is_ok());
        assert!(kind_permitted(kind, Some("otherpack")).is_err());
        assert!(kind_permitted(kind, None).is_err());
        assert!(kind_permitted(GuiKind::Furnace, None).is_ok());
        assert!(kind_permitted(GuiKind::Title, Some("anypack")).is_ok());
    }
}
