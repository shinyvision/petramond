use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use petramond_ui::contract::{self, slot_semantics_issues, EngineCatalog};
use petramond_ui::Document;
use petramond_world::container::{SlotFilter, SlotSpec};
use petramond_world::furnace::{FURNACE_SLOTS, SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use petramond_world::gui_state::GuiKind;
use petramond_world::item::ItemTag;

pub const DOCUMENTS_DIR: &str = "ui/documents";

struct SlotTable {
    by_kind: HashMap<GuiKind, Arc<Vec<SlotSpec>>>,
    // Every node id the loaded documents declare: the only widget ids a
    // client's menu click may name, so wire input never grows the intern table.
    widgets: HashSet<&'static str>,
}

static TABLE: OnceLock<SlotTable> = OnceLock::new();

fn table() -> &'static SlotTable {
    TABLE.get_or_init(load)
}

pub fn slot_specs_for_kind(kind: GuiKind) -> Arc<Vec<SlotSpec>> {
    table().by_kind.get(&kind).cloned().unwrap_or_default()
}

pub fn declared_widget(id: &str) -> Option<&'static str> {
    if let Some(hit) = table().widgets.get(id) {
        return Some(hit);
    }
    #[cfg(any(test, feature = "test-support"))]
    if let Some(hit) = TEST_WIDGETS.lock().unwrap().iter().find(|w| **w == id) {
        return Some(hit);
    }
    None
}

/// Widget ids a test's document-less mod GUI answers to, as if a document declared them.
#[cfg(any(test, feature = "test-support"))]
static TEST_WIDGETS: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

#[cfg(any(test, feature = "test-support"))]
pub fn declare_widget_for_test(id: &str) {
    let mut widgets = TEST_WIDGETS.lock().unwrap();
    if !widgets.contains(&id) {
        widgets.push(petramond_world::gui_state::intern_str(id));
    }
}

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

fn furnace_slot_specs() -> Vec<SlotSpec> {
    let mut specs = vec![SlotSpec::default(); FURNACE_SLOTS];
    specs[SLOT_INPUT].accepts = vec![SlotFilter::Tag(ItemTag::SMELTABLE)];
    specs[SLOT_FUEL].accepts = vec![SlotFilter::Tag(ItemTag::FUEL)];
    specs[SLOT_OUTPUT].take_only = true;
    specs
}

pub(crate) struct ItemTags;

impl EngineCatalog for ItemTags {
    fn item_tag_exists(&self, name: &str) -> bool {
        ItemTag::lookup(name).is_some()
    }
}

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
    let mut widgets = HashSet::new();
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
        doc.root.visit(&mut |node| {
            if let Some(id) = &node.id {
                widgets.insert(petramond_world::gui_state::intern_str(id));
            }
        });
        match doc_container_specs(&doc) {
            Ok(specs) if specs.is_empty() => {}
            Ok(specs) => {
                by_kind.insert(kind, Arc::new(specs));
            }
            Err(e) => log::warn!("container slots: ignoring {} — {e}", path.display()),
        }
    }
    by_kind.insert(GuiKind::Furnace, Arc::new(furnace_slot_specs()));
    SlotTable { by_kind, widgets }
}

#[cfg(test)]
mod tests {
    use super::*;

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
