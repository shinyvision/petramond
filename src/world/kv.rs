//! Persistent mod data on the world (WIKI/modding.md Phase 3b): the world KV
//! map (rides `level.dat`, restored at session open) and the per-cell section
//! KV accessors (each cell's entries ride its section's save record) — plus
//! the transient Phase 5 GUI-session state map.
//!
//! Namespacing (`mod_id:key`, own-prefix writes) is enforced at the HostCall
//! boundary (`modding::host`), not here — engine/test code may use any key.
//! GUI state keys are mod-local by design (the map belongs to one session).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::gui::{GuiStateMap, GuiValue};

use super::store::World;

impl World {
    /// The whole world KV map, for the save encoder (deterministic iteration —
    /// it is a BTreeMap on purpose).
    #[inline]
    pub fn mod_kv(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.mod_kv
    }

    #[inline]
    pub fn mod_kv_get(&self, key: &str) -> Option<&[u8]> {
        self.mod_kv.get(key).map(Vec::as_slice)
    }

    pub fn mod_kv_set(&mut self, key: String, value: Vec<u8>) {
        self.mod_kv.insert(key, value);
    }

    /// Remove `key`; returns whether it was present.
    pub fn mod_kv_remove(&mut self, key: &str) -> bool {
        self.mod_kv.remove(key).is_some()
    }

    /// Replace the whole map — the session-open restore from `level.dat`.
    pub fn set_mod_kv(&mut self, map: BTreeMap<String, Vec<u8>>) {
        self.mod_kv = map;
    }

    /// A cell's KV entry at world coords, or `None` when absent or the owning
    /// section is unloaded (unloaded data stays on disk untouched).
    pub fn cell_kv_get(&self, wx: i32, wy: i32, wz: i32, key: &str) -> Option<&[u8]> {
        let (s, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        s.cell_kv_get(lx, ly, lz, key)
    }

    /// Store a cell KV entry; marks the section modified so the data persists.
    /// `false` = the owning section is unloaded / out of range (nothing stored).
    pub fn cell_kv_set(&mut self, wx: i32, wy: i32, wz: i32, key: String, value: Vec<u8>) -> bool {
        let Some((s, lx, ly, lz)) = self.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        s.cell_kv_set(lx, ly, lz, key, value);
        s.modified = true;
        true
    }

    /// Remove a cell KV entry; returns whether it was present. A removal marks
    /// the section modified so the stale on-disk record is rewritten.
    pub fn cell_kv_remove(&mut self, wx: i32, wy: i32, wz: i32, key: &str) -> bool {
        let Some((s, lx, ly, lz)) = self.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        let removed = s.cell_kv_remove(lx, ly, lz, key);
        if removed {
            s.modified = true;
        }
        removed
    }

    // ---- Phase 5: the open mod GUI session's state map ----------------------

    #[inline]
    pub fn gui_state_get(&self, key: &str) -> Option<&GuiValue> {
        self.gui_state.get(key)
    }

    /// Write a session state key (tick-side; copy-on-write against any
    /// outstanding frame snapshot — at most one clone per snapshot taken).
    pub fn gui_state_set(&mut self, key: String, value: GuiValue) {
        Arc::make_mut(&mut self.gui_state).insert(key, value);
    }

    /// Reset the map for a fresh GUI session (called by the menu funnel on
    /// open AND close, so a session can never read a predecessor's values).
    pub fn gui_state_clear(&mut self) {
        if !self.gui_state.is_empty() {
            self.gui_state = crate::gui::empty_gui_state();
        }
    }

    /// A shared read snapshot for the frame (a refcount bump; never a copy).
    #[inline]
    pub fn gui_state_snapshot(&self) -> Arc<GuiStateMap> {
        self.gui_state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI state session contract: set/get round-trips, clear resets to
    /// the shared empty map, and the frame snapshot is a refcount bump that
    /// tick-side writes never mutate in place (copy-on-write).
    #[test]
    fn gui_state_set_get_clear_and_snapshot_cow() {
        let mut w = World::new(1, 1);
        assert!(w.gui_state_get("wheel:angle").is_none());

        w.gui_state_set("wheel:angle".into(), GuiValue::F32(1.5));
        assert_eq!(w.gui_state_get("wheel:angle"), Some(&GuiValue::F32(1.5)));

        // A held frame snapshot keeps its values across later writes.
        let snap = w.gui_state_snapshot();
        w.gui_state_set("wheel:angle".into(), GuiValue::F32(2.0));
        w.gui_state_set("wheel:result".into(), GuiValue::Str("stick".into()));
        assert_eq!(snap.get("wheel:angle"), Some(&GuiValue::F32(1.5)));
        assert_eq!(snap.get("wheel:result"), None);
        assert_eq!(w.gui_state_get("wheel:angle"), Some(&GuiValue::F32(2.0)));

        // Unchanged between snapshots = the same allocation (no per-frame copy).
        let a = w.gui_state_snapshot();
        let b = w.gui_state_snapshot();
        assert!(Arc::ptr_eq(&a, &b));

        w.gui_state_clear();
        assert!(w.gui_state_get("wheel:angle").is_none());
        assert!(w.gui_state_get("wheel:result").is_none());
    }
}
