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
use petramond_ui::{DocClass, Document, Node, NodeKind, SlotContract};
use petramond_world::container::{SlotSpec, MAX_CONTAINER_SLOTS, MAX_SLOT_FILTERS};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Where GUI documents live, relative to the asset roots.
const DOCUMENTS_DIR: &str = "ui/documents";

pub struct DocEntry {
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
pub struct DocImageRef {
    pub name: String,
    pub path: PathBuf,
    pub size: (u32, u32),
}

/// A cheap handle to one loaded document.
#[derive(Clone)]
pub struct DocRef {
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

/// The document for `kind`, if one is loaded. A CRAFTING STATION kind
/// without its own document — a pack workbench, or the engine's furniture
/// workbench — is backed by the crafting table's: a station runs the
/// ordinary crafting session, so its screen is the ordinary crafting screen
/// unless a dedicated document ships.
pub fn doc_for(kind: GuiKind) -> Option<DocRef> {
    doc_entry_for(kind).or_else(|| {
        // Only stations WITHOUT a document of their own fall back — the
        // inventory and table own their browser screens, and a validation
        // failure there must stay a loud missing-document failure, not a
        // silent swap to the other's layout.
        use petramond_world::crafting::CraftingStation;
        CraftingStation::of_kind(kind)
            .is_some_and(|s| s != CraftingStation::Inventory && s != CraftingStation::CraftingTable)
            .then(|| doc_entry_for(GuiKind::CraftingTable))
            .flatten()
    })
}

fn doc_entry_for(kind: GuiKind) -> Option<DocRef> {
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

/// Every LOADED document's kind key with the number of `container` role slots
/// it declares — the developer-tool view of "was this pack's document
/// accepted?".
///
/// A rejected document is the one failure in this area with no symptom: the
/// kind still opens, the specs come back empty, and the pack machine silently
/// becomes plain storage. Nothing else surfaces it without launching the game.
pub fn loaded_documents() -> Vec<(&'static str, usize)> {
    let mut guard = REGISTRY.lock().expect("gui document registry");
    let registry = guard.get_or_insert_with(load);
    let mut out: Vec<(&'static str, usize)> = registry
        .entries
        .iter()
        .filter_map(|e| {
            Some((
                petramond_world::gui_state::kind_key(e.kind)?,
                e.container_slots.len(),
            ))
        })
        .collect();
    out.sort_unstable();
    out
}

/// A document's `container` role slot semantics, in-role index order — for
/// ENGINE kinds as well as mod ones (the chest's 27 unfiltered cells come from
/// here; only the furnace's are hardcoded, in `ContainerMenu::slot_specs`).
/// Empty for widgets-only mod GUIs and unknown kinds.
pub fn container_slot_specs(kind: GuiKind) -> Arc<Vec<SlotSpec>> {
    doc_for(kind).map(|d| d.container_slots).unwrap_or_default()
}

/// The slot contract a MOD document earns from its own declarations: mod
/// kinds may declare generic `container` slots (engine-backed mod-owned
/// storage, capped), plus the standard `player_inv`/`hotbar` grids with the
/// engine counts. Any other role is refused — a mod document can never name
/// an engine block-entity's roles. `Err` skips the document loudly at load.
fn document_contract_for(doc: &Document) -> Result<SlotContract, String> {
    // The engine grids' sizes come from the inventory layout itself.
    const MAIN_GRID: usize =
        petramond_world::inventory::TOTAL_SLOTS - petramond_world::inventory::HOTBAR_LEN;
    const HOTBAR: usize = petramond_world::inventory::HOTBAR_LEN;
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
            "off_hand" if count == 1 => {}
            "player_inv" | "hotbar" | "off_hand" => {
                return Err(format!(
                    "role '{role}' declares {count} slots; the engine grids are \
                     player_inv:{MAIN_GRID}, hotbar:{HOTBAR}, and off_hand:1"
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

/// A mod document's `container` slot semantics in in-role index order.
///
/// A TAG name is checked against the item-tag registry via the non-interning
/// QUERY lookup — the interning resolve would register a misspelled tag as a
/// fresh empty one and the slot would silently accept nothing. A DATA key is
/// checked for being NAMESPACED and nothing else: data keys have no
/// declaration anywhere (a row states one by carrying it), so "no row carries
/// it yet" is a pack shipping its slot before its rows, not an error. Both
/// failures are document errors (`Err` skips it loudly).
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
        if cell.accepts.len() > MAX_SLOT_FILTERS {
            return Err(format!(
                "a container slot declares {} accepts filters; the cap is {MAX_SLOT_FILTERS} \
                 (the runtime accepts mask spends one bit per filter)",
                cell.accepts.len()
            ));
        }
        let mut filters = Vec::new();
        for accept in &cell.accepts {
            filters.push(resolve_slot_filter(accept)?);
        }
        specs.push(SlotSpec {
            accepts: filters,
            take_only: cell.take_only,
            accepts_bind: cell.accepts_bind.as_deref().map(super::intern_str),
        });
    }
    Ok(specs)
}

/// One authored `accepts` entry → the runtime filter.
fn resolve_slot_filter(
    accept: &petramond_ui::doc::Accept,
) -> Result<petramond_world::container::SlotFilter, String> {
    match accept {
        petramond_ui::doc::Accept::Tag(name) => petramond_world::item::ItemTag::lookup(name)
            .map(petramond_world::container::SlotFilter::Tag)
            .ok_or_else(|| format!("unknown item tag '{name}' in a slot's accepts")),
        petramond_ui::doc::Accept::Data { data } => {
            if !petramond_world::registry::is_namespaced(data) {
                return Err(format!(
                    "slot accepts data key '{data}': data keys must be namespaced ('mod_id:name')"
                ));
            }
            Ok(petramond_world::container::SlotFilter::Data(
                super::intern_str(data),
            ))
        }
    }
}

/// The engine's slot expectations per kind. Mod kinds derive their contract
/// from their own document via `document_contract_for`; shell kinds carry no
/// role slots.
pub fn contract_for(kind: GuiKind) -> SlotContract {
    match kind {
        GuiKind::Chest => SlotContract::new(&[
            ("container", crate::world::chest::CHEST_SLOTS),
            ("player_inv", 27),
            ("hotbar", 9),
        ]),
        GuiKind::Inventory => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("off_hand", 1),
            ("craft_result", 1),
        ]),
        GuiKind::CraftingTable => {
            SlotContract::new(&[("player_inv", 27), ("hotbar", 9), ("craft_result", 1)])
        }
        GuiKind::Furnace => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            // Input, fuel, output — in `SLOT_INPUT`/`SLOT_FUEL`/`SLOT_OUTPUT`
            // order, since the in-role index IS the container index.
            ("container", petramond_world::furnace::FURNACE_SLOTS),
        ]),
        GuiKind::Creative => SlotContract::new(&[("hotbar", 9)]),
        GuiKind::Hotbar => SlotContract::new(&[("hotbar", 9), ("off_hand", 1)]),
        GuiKind::Demo => SlotContract::new(&[("demo_slots", 9)]),
        _ => SlotContract::default(),
    }
}

/// The mod-kind ownership rule, shared with the baked path: a namespaced
/// document kind must ship from the pack owning the namespace.
fn kind_permitted(kind: GuiKind, pack_id: Option<&str>) -> Result<(), String> {
    if !kind.is_registered() {
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

/// Every image a document statically names — on `image`, `rotimage`, and
/// image-backed `button` nodes alike — resolved beside the document in
/// first-reference order. `Err` rejects the document: a statically named
/// image whose file is missing used to only skip its quad, which no pack
/// author ever saw until the screen drew wrong.
fn collect_doc_images(doc: &Document, dir: &std::path::Path) -> Result<Vec<DocImageRef>, String> {
    // First-reference order (the order feeds TexId::DocImage), keeping the
    // first SEEN frames grid so an unframed reference cannot hide a framed
    // one from the sheet check below.
    let mut refs: Vec<(String, Option<[u32; 2]>)> = Vec::new();
    doc.root.visit(&mut |node| {
        let (name, frames) = match &node.kind {
            petramond_ui::NodeKind::Image { image, frames, .. } => (image.as_str(), *frames),
            petramond_ui::NodeKind::Rotimage { image, .. } => (image.as_str(), None),
            petramond_ui::NodeKind::Button {
                image: Some(image),
                frames,
                ..
            } => (image.as_str(), *frames),
            _ => return,
        };
        // An empty static name is the runtime-bound pattern (`bind.image`
        // supplies the art, e.g. the world-settings pack icons) — there is
        // no file to resolve beside the document.
        if name.is_empty() {
            return;
        }
        match refs.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => {
                if slot.1.is_none() {
                    slot.1 = frames;
                }
            }
            None => refs.push((name.to_string(), frames)),
        }
    });
    let mut images = Vec::with_capacity(refs.len());
    for (name, frames) in refs {
        let path = dir.join(&name);
        let size =
            image::image_dimensions(&path).map_err(|_| format!("names missing art {name}"))?;
        if let Some(frames) = frames {
            validate_frame_sheet(&name, size, frames)?;
        }
        images.push(DocImageRef { name, path, size });
    }
    Ok(images)
}

/// A framed sheet is uploaded whole and ONE frame is drawn per node, so the
/// grid must divide the image exactly and both must stay inside the shared
/// GUI image bounds — a bad grid mis-slices every frame of the sheet.
fn validate_frame_sheet(name: &str, size: (u32, u32), frames: [u32; 2]) -> Result<(), String> {
    let [cols, rows] = frames;
    let (w, h) = size;
    if cols == 0 || rows == 0 || w % cols != 0 || h % rows != 0 {
        return Err(format!(
            "image {name} is {w}x{h}, which the frames grid {cols}x{rows} does not divide evenly"
        ));
    }
    if cols * rows > mod_api::GUI_IMAGE_MAX_FRAMES {
        return Err(format!(
            "image {name} declares {} frames; the cap is {}",
            cols * rows,
            mod_api::GUI_IMAGE_MAX_FRAMES
        ));
    }
    if w > mod_api::GUI_IMAGE_MAX_SIDE || h > mod_api::GUI_IMAGE_MAX_SIDE {
        return Err(format!(
            "image {name} is {w}x{h}; the GUI image side cap is {}",
            mod_api::GUI_IMAGE_MAX_SIDE
        ));
    }
    Ok(())
}

/// The standard slot-tooltip chrome every CONTAINER document carries: hovering
/// a filled slot floats the stack's item name (plus the item's optional `info`
/// line) at the pointer, fed by the `item_tip_*` keys the host populates for
/// every menu kind. Injected at load so every GUI that shows slots — engine
/// containers and pack machine panels alike — has it without shipping the
/// node by hand: one definition, never per-document drift. A document that
/// binds `show_item_tip` itself keeps its own chrome instead.
fn inject_item_tooltip(doc: &mut Document) {
    /// Keep in sync with the `item_tip_*` keys in `assets/ui/bindings.json`.
    const STANDARD: &str = r#"{
        "type": "tooltip",
        "style": "panel.inset",
        "layout": { "abs": { "x": 4, "y": 4 }, "pad": [3, 2, 3, 3], "gap": 2, "align": "stretch" },
        "bind": { "visible": "show_item_tip" },
        "children": [
            { "type": "label", "small": true, "wrap": true, "bind": { "text": "item_tip_name" } },
            { "type": "label", "small": true, "style": "label.muted", "wrap": true,
              "bind": { "text": "item_tip_info", "visible": "item_tip_has_info" } },
            { "type": "label", "small": true, "wrap": true,
              "bind": { "text": "item_tip_instance", "visible": "item_tip_instance" } },
            { "type": "row", "layout": { "gap": 4 }, "children": [
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon0", "visible": "item_tip_icon0" } },
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon1", "visible": "item_tip_icon1" } },
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon2", "visible": "item_tip_icon2" } },
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon3", "visible": "item_tip_icon3" } }
            ] }
        ]
    }"#;
    fn binds_item_tip(node: &Node) -> bool {
        node.bind.visible.as_deref() == Some("show_item_tip")
            || node.children.iter().any(binds_item_tip)
    }
    if doc.class != DocClass::Container || binds_item_tip(&doc.root) {
        return;
    }
    let mut node: Node = serde_json::from_str(STANDARD).expect("standard item tooltip node parses");
    node.layout.max_w = Some(ITEM_TIP_MAX_W);
    doc.root.children.push(node);
}

/// Width cap (logical px) of the injected item tooltip. A floating node has
/// nothing to constrain its natural width, so one long bound string (a pack's
/// recipe name) would make a panel as wide as the screen; under the cap the
/// wrapped labels break at it instead. 200 keeps the panel beside the pointer
/// at the tightest 320 px viewport — the ceiling the documents overflow test
/// holds every floating panel to.
pub const ITEM_TIP_MAX_W: i32 = 200;

/// The injected slot tooltip's cap. Its lines are two palette-coloured spans
/// side by side ("Great  Diamond Tip") at secondary size, which need the extra
/// width over the item tip before the second span starts ellipsizing; still
/// well inside the 320 px viewport with the pointer offset.
pub const SLOT_TIP_MAX_W: i32 = 240;

/// How many LINES the injected slot tooltip shows, and how many coloured
/// SPANS each line carries. THE statement of the tooltip's shape: the node is
/// generated from these and the host publishes exactly the keys
/// [`slot_tip_keys`] names, so widening the tooltip is these numbers alone.
pub const SLOT_TIP_LINES: usize = 3;
pub const SLOT_TIP_SPANS: usize = 2;

/// The `(text, palette)` state keys one span of the injected slot tooltip
/// binds — the naming authority for the injected node and for the host that
/// populates it alike.
pub fn slot_tip_keys(line: usize, span: usize) -> (String, String) {
    (
        format!("slot_tip_l{line}s{span}"),
        format!("slot_tip_c{line}s{span}"),
    )
}

/// The key whose `Bool` reveals the injected slot tooltip. Named here with
/// the span keys because the host that sets it lives in another crate — a
/// literal on each side is a rename that silently stops the tip appearing.
pub const SHOW_SLOT_TIP: &str = "show_slot_tip";

/// The BOUND slot tooltip: a machine may publish per-slot tooltip LINES
/// under the gui-state key `slot{index}:tip` — lines separated by newline,
/// each line up to [`SLOT_TIP_SPANS`] SPANS separated by tab, each span a
/// `palette|text` pair (the palette half naming a theme palette entry) — and
/// hovering that slot while it holds NO stack floats them at the pointer (the
/// item tip owns a hovered stack, so the two never show together). Spans are
/// what colour one WORD of a line without painting the rest (Rachel,
/// 2026-08-10: "Great" green, "Diamond Tip" plain). Injected like the item
/// tip: one definition for every container document; binding
/// [`SHOW_SLOT_TIP`] itself keeps a document's own chrome.
fn inject_slot_tooltip(doc: &mut Document) {
    const CHROME: &str = r#"{
        "type": "tooltip",
        "style": "panel.inset",
        "layout": { "abs": { "x": 4, "y": 4 }, "pad": [3, 2, 3, 3], "gap": 2, "align": "stretch" },
        "bind": { }
    }"#;
    fn binds_slot_tip(node: &Node) -> bool {
        node.bind.visible.as_deref() == Some(SHOW_SLOT_TIP)
            || node.children.iter().any(binds_slot_tip)
    }
    if doc.class != DocClass::Container || binds_slot_tip(&doc.root) {
        return;
    }
    let mut tip: Node = serde_json::from_str(CHROME).expect("standard slot tooltip node parses");
    tip.layout.max_w = Some(SLOT_TIP_MAX_W);
    tip.bind.visible = Some(SHOW_SLOT_TIP.to_owned());
    tip.children = (0..SLOT_TIP_LINES)
        .map(|line| {
            let mut row = Node::leaf(NodeKind::Row);
            // A line with no first span has nothing to show.
            row.bind.visible = Some(slot_tip_keys(line, 0).0);
            row.children = (0..SLOT_TIP_SPANS)
                .map(|span| slot_tip_span(line, span))
                .collect();
            row
        })
        .collect();
    doc.root.children.push(tip);
}

/// One span of one slot-tooltip line: secondary-sized text whose colour is
/// state (`bind.palette`).
fn slot_tip_span(line: usize, span: usize) -> Node {
    let (text, palette) = slot_tip_keys(line, span);
    let mut label = Node::leaf(NodeKind::Label {
        text: None,
        wrap: false,
        scale: 1,
        small: true,
    });
    label.bind.text = Some(text);
    label.bind.palette = Some(palette);
    label
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
    for (dir, pack_id) in petramond_world::assets::layer_dirs_with_ids(DOCUMENTS_DIR) {
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
        let mut doc = match Document::from_json(&text) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("gui: ignoring {} — {e}", found.json.display());
                continue;
            }
        };
        inject_item_tooltip(&mut doc);
        inject_slot_tooltip(&mut doc);
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
        let contract = if kind.is_registered() {
            match document_contract_for(&doc) {
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
        // their pixel sizes for layout naturals. Bad art — a missing file, a
        // frame grid that does not divide its sheet — rejects the document
        // loudly like any other validation failure.
        let images = match collect_doc_images(&doc, &found.dir) {
            Ok(images) => images,
            Err(e) => {
                eprintln!("gui: ignoring {} — {e}", found.json.display());
                continue;
            }
        };
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
mod tests;
