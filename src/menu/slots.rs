//! Container slot contracts: what each slot of every container kind admits
//! (item filters), whether it is a take-only output, and how many there are.
//!
//! These are GAMEPLAY rules — player menus, automated transfers and mod
//! container access all obey them — so the sim owns them here, apart from
//! the GUI layer. The table is built ONCE, the first time anything asks, and
//! is immutable afterwards: no lock on the read path, no hot reload, so no
//! layout edit can change a slot's rules mid-session. Pack containers
//! declare their semantics beside their layout (the `container` slots'
//! `accepts` / `take_only` in their `*.gui.json`); this loader reads only
//! those declarations — the layout, theme and art are the GUI registry's
//! business ([`crate::gui::documents`]), which validates each document
//! against this table. The furnace's semantics are engine machine state and
//! are fixed here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use petramond_ui::contract::{self, slot_semantics_issues, EngineCatalog};
use petramond_ui::Document;
use petramond_world::container::{SlotFilter, SlotSpec};
use petramond_world::furnace::{FURNACE_SLOTS, SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use petramond_world::gui_state::GuiKind;
use petramond_world::item::ItemTag;

/// Where container kinds declare their slots, relative to the asset roots
/// (shared with the GUI document registry, which reads the same files for
/// their layout).
pub const DOCUMENTS_DIR: &str = "ui/documents";

/// Every container kind's slot specs.
struct SlotTable {
    by_kind: HashMap<GuiKind, Arc<Vec<SlotSpec>>>,
}

static TABLE: OnceLock<SlotTable> = OnceLock::new();

fn table() -> &'static SlotTable {
    TABLE.get_or_init(load)
}

/// Slot admission for `kind`, shared by player menus, automated container
/// transfers and mod container access. Empty for kinds without container
/// slots (widgets-only mod GUIs, unknown kinds).
pub fn slot_specs_for_kind(kind: GuiKind) -> Arc<Vec<SlotSpec>> {
    table().by_kind.get(&kind).cloned().unwrap_or_default()
}

/// Every kind with container slots, as `(kind key, slot count)`, sorted.
pub fn declared_kinds() -> Vec<(&'static str, usize)> {
    let mut out: Vec<_> = table()
        .by_kind
        .iter()
        .filter_map(|(kind, specs)| {
            Some((petramond_world::gui_state::kind_key(*kind)?, specs.len()))
        })
        .collect();
    out.sort_unstable();
    out
}

/// The furnace's semantics: a smeltable-filtered input, a fuel-filtered fuel
/// slot, and a take-only output, in the `SLOT_INPUT`/`SLOT_FUEL`/`SLOT_OUTPUT`
/// index convention.
fn furnace_slot_specs() -> Vec<SlotSpec> {
    let mut specs = vec![SlotSpec::default(); FURNACE_SLOTS];
    specs[SLOT_INPUT].accepts = vec![SlotFilter::Tag(ItemTag::SMELTABLE)];
    specs[SLOT_FUEL].accepts = vec![SlotFilter::Tag(ItemTag::FUEL)];
    specs[SLOT_OUTPUT].take_only = true;
    specs
}

/// The item-tag registry as the shared validator sees it. The check stays on
/// the non-interning QUERY lookup: the interning resolve would register a
/// misspelled tag as a fresh empty one and the slot would silently accept
/// nothing.
pub(crate) struct ItemTags;

impl EngineCatalog for ItemTags {
    fn item_tag_exists(&self, name: &str) -> bool {
        ItemTag::lookup(name).is_some()
    }
}

/// A document's `container` slot semantics in in-role index order.
///
/// The rules (semantics only on `container` slots, the filter cap, tag
/// existence, namespaced data keys) are the shared
/// [`slot_semantics_issues`]; this resolves the authored filters to runtime
/// ones once they pass. `Err` rejects the declaration loudly.
pub(crate) fn doc_container_specs(doc: &Document) -> Result<Vec<SlotSpec>, String> {
    let issues = slot_semantics_issues(doc, &ItemTags);
    if !issues.is_empty() {
        return Err(issues.join("; "));
    }
    let mut specs = Vec::new();
    for cell in doc.slot_semantics() {
        if cell.role != "container" {
            continue;
        }
        let accepts = cell
            .accepts
            .iter()
            .map(resolve_slot_filter)
            .collect::<Result<Vec<_>, _>>()?;
        specs.push(SlotSpec {
            accepts,
            take_only: cell.take_only,
            accepts_bind: cell
                .accepts_bind
                .as_deref()
                .map(petramond_world::gui_state::intern_str),
        });
    }
    Ok(specs)
}

/// One validated `accepts` entry → the runtime filter.
fn resolve_slot_filter(accept: &petramond_ui::doc::Accept) -> Result<SlotFilter, String> {
    match accept {
        petramond_ui::doc::Accept::Tag(name) => ItemTag::lookup(name)
            .map(SlotFilter::Tag)
            .ok_or_else(|| format!("unknown item tag '{name}' in a slot's accepts")),
        petramond_ui::doc::Accept::Data { data } => Ok(SlotFilter::Data(
            petramond_world::gui_state::intern_str(data),
        )),
    }
}

/// The declaring files, overlaid by file name: base roots first, packs
/// after — the last copy of a name wins, exactly as the GUI registry
/// resolves the same files.
fn declaring_files() -> Vec<(PathBuf, Option<String>)> {
    let mut files: Vec<(String, PathBuf, Option<String>)> = Vec::new();
    for (dir, pack_id) in petramond_world::assets::layer_dirs_with_ids(DOCUMENTS_DIR) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".gui.json") {
                continue;
            }
            let found = (name, entry.path(), pack_id.clone());
            match files.iter_mut().find(|(n, ..)| *n == found.0) {
                Some(slot) => *slot = found,
                None => files.push(found),
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
        .into_iter()
        .map(|(_, path, pack)| (path, pack))
        .collect()
}

fn load() -> SlotTable {
    let mut by_kind = HashMap::new();
    for (path, pack_id) in declaring_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let doc = match Document::from_json(&text) {
            Ok(doc) => doc,
            Err(e) => {
                log::warn!("container slots: ignoring {} — {e}", path.display());
                continue;
            }
        };
        if let Err(e) = contract::kind_permitted(&doc.kind, pack_id.as_deref()) {
            log::warn!("container slots: ignoring {} — {e}", path.display());
            continue;
        }
        let Some(kind) = petramond_world::gui_state::intern_kind(&doc.kind) else {
            log::warn!(
                "container slots: ignoring {} — unknown kind '{}'",
                path.display(),
                doc.kind
            );
            continue;
        };
        match doc_container_specs(&doc) {
            Ok(specs) if specs.is_empty() => {}
            Ok(specs) => {
                by_kind.insert(kind, Arc::new(specs));
            }
            Err(e) => log::warn!("container slots: ignoring {} — {e}", path.display()),
        }
    }
    by_kind.insert(GuiKind::Furnace, Arc::new(furnace_slot_specs()));
    SlotTable { by_kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table answers from one immutable build: the engine kinds are in
    /// it, a kind without container slots reads empty, and repeated reads
    /// share the same allocation (no reload behind the sim's back).
    #[test]
    fn the_table_is_built_once_and_covers_the_engine_containers() {
        let declared = declared_kinds();
        assert!(declared
            .iter()
            .any(|(key, n)| key.ends_with("chest") && *n > 0));
        assert!(declared
            .iter()
            .any(|(key, n)| key.ends_with("furnace") && *n == FURNACE_SLOTS));
        assert!(slot_specs_for_kind(GuiKind::Other).is_empty());
        assert!(Arc::ptr_eq(
            &slot_specs_for_kind(GuiKind::Furnace),
            &slot_specs_for_kind(GuiKind::Furnace)
        ));
    }
}
