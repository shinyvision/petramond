//! The runtime GUI-kind registry (WIKI/modding.md Phase 5).
//!
//! [`GuiKind`] follows the Phase 2a newtype pattern: engine kinds are frozen
//! low ids behind consts named exactly like the old enum variants, so existing
//! expressions and const match patterns still compile; mod packs ADD kinds by
//! declaring a namespaced `kind: "mod_id:name"` in a GUI document (or an
//! `open_gui` block interaction), which interns the next free id. Kind ids are
//! session-scoped and never persisted — the stable identity is the key string
//! (events and the ABI speak keys, never ids).
//!
//! Unlike blocks/items there is no bootstrap ordering constraint (nothing
//! cross-references kinds by id), so registration is on-demand behind a mutex:
//! the first loader to name a key assigns its id, every later resolution
//! agrees within the process.

use std::sync::Mutex;

/// Which GUI a document describes / a screen draws. Engine kinds are the
/// consts below; mod kinds are interned at load. [`GuiKind::Other`] is the
/// not-a-container sentinel — it is never registered and owns no document.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct GuiKind(u8);

#[allow(non_upper_case_globals)]
impl GuiKind {
    pub const Chest: GuiKind = GuiKind(0);
    pub const Inventory: GuiKind = GuiKind(1);
    pub const CraftingTable: GuiKind = GuiKind(2);
    pub const Furnace: GuiKind = GuiKind(3);
    pub const Hotbar: GuiKind = GuiKind(4);
    pub const FurnitureWorkbench: GuiKind = GuiKind(5);
    // Document-backed shell screens (the GUI-document runtime; appended, ids
    // are session-scoped so extending the table is safe).
    pub const Title: GuiKind = GuiKind(6);
    pub const WorldSelect: GuiKind = GuiKind(7);
    pub const WorldSettings: GuiKind = GuiKind(8);
    pub const CreateWorld: GuiKind = GuiKind(9);
    pub const DeleteWorld: GuiKind = GuiKind(10);
    pub const Pause: GuiKind = GuiKind(11);
    /// Dev-only widget-catalog demo screen.
    pub const Demo: GuiKind = GuiKind(12);
    /// The sleep overlay (dark fade + "Leave bed"), over a live simulation.
    pub const Sleep: GuiKind = GuiKind(13);
    /// The death screen ("You died": respawn / save-and-quit).
    pub const Death: GuiKind = GuiKind(14);
    /// The not-a-container sentinel; compares equal to no registered kind.
    pub const Other: GuiKind = GuiKind(u8::MAX);

    /// Whether this is a pack-registered (namespaced) kind, as opposed to an
    /// engine kind or the [`Other`](GuiKind::Other) sentinel.
    #[inline]
    pub fn is_mod(self) -> bool {
        self.0 as usize >= ENGINE_GUI_KIND_NAMES.len() && self != GuiKind::Other
    }
}

/// Engine kind keys, index == frozen id. Append-only, like every engine name
/// table.
const ENGINE_GUI_KIND_NAMES: [&str; 15] = [
    "llama:chest",
    "llama:inventory",
    "llama:crafting_table",
    "llama:furnace",
    "llama:hotbar",
    "llama:furniture_workbench",
    "llama:title",
    "llama:world_select",
    "llama:world_settings",
    "llama:create_world",
    "llama:delete_world",
    "llama:pause",
    "llama:demo",
    "llama:sleep",
    "llama:death",
];

/// Registered mod kinds cap out below the `Other` sentinel; in practice a
/// session has a handful.
const MAX_KINDS: usize = 250;

/// Interned strings shared by GUI defs and ids: registered kind keys, widget
/// ids, and sprite (image/tag) keys — all `&'static str` so the types staying
/// `Copy + Hash` ([`GuiKind`], `MenuSlot::Widget`, the renderer's texture
/// keys) can carry them. Bounded: manifests load once; tests add a handful.
static INTERNED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Mod kind keys in registration order; index + engine count == id.
static MOD_KINDS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Deduplicate `s` into a `'static` string (see [`INTERNED`]).
pub(crate) fn intern_str(s: &str) -> &'static str {
    let mut interned = INTERNED.lock().unwrap();
    if let Some(hit) = interned.iter().find(|i| **i == s) {
        return hit;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    interned.push(leaked);
    leaked
}

/// Resolve `key` to its kind, REGISTERING a namespaced key on first sight.
/// Engine `llama:*` names map to their consts; a new bare name or unknown
/// `llama:*` key is `None`.
pub(crate) fn intern_kind(key: &str) -> Option<GuiKind> {
    if let Some(i) = ENGINE_GUI_KIND_NAMES.iter().position(|n| *n == key) {
        return Some(GuiKind(i as u8));
    }
    if crate::registry::namespace(key) == Some(crate::registry::ENGINE_NAMESPACE) {
        return None;
    }
    if !crate::registry::is_namespaced(key) {
        return None;
    }
    let mut kinds = MOD_KINDS.lock().unwrap();
    if let Some(i) = kinds.iter().position(|n| *n == key) {
        return Some(GuiKind((ENGINE_GUI_KIND_NAMES.len() + i) as u8));
    }
    if ENGINE_GUI_KIND_NAMES.len() + kinds.len() >= MAX_KINDS {
        log::error!("gui kind registry full; cannot register '{key}'");
        return None;
    }
    kinds.push(intern_str(key));
    Some(GuiKind(
        (ENGINE_GUI_KIND_NAMES.len() + kinds.len() - 1) as u8,
    ))
}

/// Resolve `key` WITHOUT registering (the `GuiOpen` HostCall path: opening a
/// kind nothing declared is a mod bug, not a registration).
pub(crate) fn resolve_kind(key: &str) -> Option<GuiKind> {
    if let Some(i) = ENGINE_GUI_KIND_NAMES.iter().position(|n| *n == key) {
        return Some(GuiKind(i as u8));
    }
    let kinds = MOD_KINDS.lock().unwrap();
    kinds
        .iter()
        .position(|n| *n == key)
        .map(|i| GuiKind((ENGINE_GUI_KIND_NAMES.len() + i) as u8))
}

/// The registered key of `kind` (`None` for [`GuiKind::Other`] / unregistered
/// ids). Events and the ABI carry this string, never the session id.
pub(crate) fn kind_key(kind: GuiKind) -> Option<&'static str> {
    let i = kind.0 as usize;
    if let Some(name) = ENGINE_GUI_KIND_NAMES.get(i) {
        return Some(name);
    }
    let kinds = MOD_KINDS.lock().unwrap();
    kinds.get(i - ENGINE_GUI_KIND_NAMES.len()).copied()
}

impl std::fmt::Debug for GuiKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Engine names come from the compiled table (works mid-bootstrap, like
        // Block's Debug); registered mod kinds print their key.
        match self.0 as usize {
            0 => write!(f, "Chest"),
            1 => write!(f, "Inventory"),
            2 => write!(f, "CraftingTable"),
            3 => write!(f, "Furnace"),
            4 => write!(f, "Hotbar"),
            5 => write!(f, "FurnitureWorkbench"),
            _ if *self == GuiKind::Other => write!(f, "Other"),
            i => match kind_key(*self) {
                Some(key) => write!(f, "GuiKind({key:?})"),
                None => write!(f, "GuiKind(#{i})"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_names_resolve_to_consts_and_mod_keys_intern_once() {
        assert_eq!(intern_kind("llama:furnace"), Some(GuiKind::Furnace));
        assert_eq!(resolve_kind("llama:hotbar"), Some(GuiKind::Hotbar));
        assert!(!GuiKind::Furnace.is_mod());
        assert!(!GuiKind::Other.is_mod());

        // A new bare name is refused; a namespaced key registers exactly once.
        assert_eq!(intern_kind("wheel"), None);
        assert_eq!(intern_kind("llama:wheel"), None);
        let a = intern_kind("kindtest:wheel").expect("namespaced key registers");
        let b = intern_kind("kindtest:wheel").unwrap();
        assert_eq!(a, b, "re-interning returns the same id");
        assert!(a.is_mod());
        assert_eq!(kind_key(a), Some("kindtest:wheel"));
        assert_eq!(resolve_kind("kindtest:wheel"), Some(a));
        // resolve_kind never registers.
        assert_eq!(resolve_kind("kindtest:never_declared"), None);
        assert_eq!(kind_key(GuiKind::Other), None);
    }

    #[test]
    fn intern_str_deduplicates() {
        let a = intern_str("kindtest:a-string");
        let b = intern_str("kindtest:a-string");
        assert!(std::ptr::eq(a, b));
    }
}
