use std::collections::BTreeMap;

use super::data::WorldData;

impl WorldData {
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

    pub fn world_kv_remove(&mut self, key: &str) -> bool {
        self.content.world_kv.remove(key).is_some()
    }

    pub fn set_world_kv(&mut self, map: BTreeMap<String, Vec<u8>>) {
        self.content.world_kv = map;
    }

    pub fn cell_kv_get(&self, wx: i32, wy: i32, wz: i32, key: &str) -> Option<&[u8]> {
        let (s, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        s.cell_kv_get(lx, ly, lz, key)
    }

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
