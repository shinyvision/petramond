//! The world's persistent key/value data: the world KV map (rides `level.dat`,
//! restored at session open) and the per-cell section KV accessors (each
//! cell's entries ride its section's save record). The day/night clock and the
//! operator list live here alongside anything a pack stores.
//!
//! Namespacing (`namespace:key`, own-prefix writes) is enforced at the HostCall
//! boundary (`modding::host`), not here — engine/test code may use any key.
//! The GUI-session state map is NOT here: it lives on the player session
//! (`ConnectedPlayer::gui_state` + the `crate::gui` state helpers).
//! (Data-half queries; the mutation/orchestration half stays in the engine crate.)

use std::collections::BTreeMap;

use super::data::WorldData;

impl WorldData {
    /// The whole world KV map, for the save encoder (deterministic iteration —
    /// it is a BTreeMap on purpose).
    #[inline]
    pub fn world_kv(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.content.world_kv
    }

    #[inline]
    pub fn world_kv_get(&self, key: &str) -> Option<&[u8]> {
        self.content.world_kv.get(key).map(Vec::as_slice)
    }

    pub fn world_kv_set(&mut self, key: String, value: Vec<u8>) {
        self.content.world_kv.insert(key, value);
    }

    /// Remove `key`; returns whether it was present.
    pub fn world_kv_remove(&mut self, key: &str) -> bool {
        self.content.world_kv.remove(key).is_some()
    }

    /// Replace the whole map — the session-open restore from `level.dat`.
    pub fn set_world_kv(&mut self, map: BTreeMap<String, Vec<u8>>) {
        self.content.world_kv = map;
    }

    /// A cell's KV entry at world coords, or `None` when absent or the owning
    /// section is unloaded (unloaded data stays on disk untouched).
    pub fn cell_kv_get(&self, wx: i32, wy: i32, wz: i32, key: &str) -> Option<&[u8]> {
        let (s, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        s.cell_kv_get(lx, ly, lz, key)
    }

    /// The number of KV entries one cell holds (0 for the common bare cell or
    /// an unloaded section) — the aggregate-cap read behind the host guard:
    /// every `BlockDelta` of the cell ships its WHOLE map, so the per-cell
    /// entry count must stay bounded like the per-entry sizes.
    pub fn cell_kv_count(&self, wx: i32, wy: i32, wz: i32) -> usize {
        let Some((s, lx, ly, lz)) = self.chunk_at_world(wx, wy, wz) else {
            return 0;
        };
        let cell = crate::chunk::section_idx(lx, ly, lz) as u16;
        s.cell_kv().get(&cell).map_or(0, |m| m.len())
    }

    pub fn cell_kv_writable(&self, wx: i32, wy: i32, wz: i32) -> bool {
        crate::chunk::SectionPos::from_world(wx, wy, wz).is_some_and(|sp| self.stream_writable(sp))
    }
}
