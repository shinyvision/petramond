//! Persistent mod data on the world (Phase 3b): the world KV
//! map (rides `level.dat`, restored at session open) and the per-cell section
//! KV accessors (each cell's entries ride its section's save record).
//!
//! Namespacing (`mod_id:key`, own-prefix writes) is enforced at the HostCall
//! boundary (`modding::host`), not here — engine/test code may use any key.
//! The GUI-session state map moved to the player session in multiplayer
//! C2c-iii (`ConnectedPlayer::gui_state` + the `crate::gui` state helpers).

use std::collections::BTreeMap;

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
    /// `false` = the owning section is unloaded / out of range / not
    /// stream-final (a write racing an in-flight saved overlay would be
    /// clobbered when it lands — refuse, like `set_block_world`).
    pub fn cell_kv_set(&mut self, wx: i32, wy: i32, wz: i32, key: String, value: Vec<u8>) -> bool {
        if !self.cell_kv_writable(wx, wy, wz) {
            return false;
        }
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
        if !self.cell_kv_writable(wx, wy, wz) {
            return false;
        }
        let Some((s, lx, ly, lz)) = self.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        let removed = s.cell_kv_remove(lx, ly, lz, key);
        if removed {
            s.modified = true;
        }
        removed
    }

    fn cell_kv_writable(&self, wx: i32, wy: i32, wz: i32) -> bool {
        crate::chunk::SectionPos::from_world(wx, wy, wz).is_some_and(|sp| self.stream_writable(sp))
    }
}
