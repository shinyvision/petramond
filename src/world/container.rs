use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::container::Container;

/// Cell data naming the loot table a generated container is stocked from: `{"table": "<key>"}`.
pub const LOOT_KEY: &str = "petramond:loot";

#[derive(serde::Deserialize)]
struct LootMarker {
    table: String,
}

impl<S: WorldSide> World<S> {
    /// Stocks a container from the loot table its [`LOOT_KEY`] cell data names, once: the data is
    /// consumed with the roll, so a looted container never refills, whether the world was saved
    /// in between or not.
    pub fn stock_loot(&mut self, pos: IVec3) {
        let Some(bytes) = self.data.cell_kv_get(pos.x, pos.y, pos.z, LOOT_KEY) else {
            return;
        };
        let marker = serde_json::from_slice::<LootMarker>(bytes);
        self.cell_kv_remove(pos.x, pos.y, pos.z, LOOT_KEY);
        let Ok(marker) = marker else {
            log::warn!("{LOOT_KEY} at {pos:?} is not {{\"table\": \"<key>\"}}");
            return;
        };
        let mut rng = petramond_worldgen::FeatureRng::positional(
            self.data.seed,
            0x1007_7ab1e,
            pos.x,
            pos.y,
            pos.z,
        );
        let Some(stacks) = petramond_world::loot::catalog().roll(&marker.table, || rng.next_u64())
        else {
            log::warn!(
                "{LOOT_KEY} at {pos:?} names unknown loot table '{}'",
                marker.table
            );
            return;
        };
        let len = self
            .container_at(pos)
            .map_or(crate::world::chest::CHEST_SLOTS, |c| c.slots.len());
        if !self.ensure_container(pos, len) {
            return;
        }
        let Some(container) = self.container_at_mut(pos) else {
            return;
        };
        let mut free: Vec<usize> = (0..container.slots.len())
            .filter(|&i| container.slots[i].is_none())
            .collect();
        for i in (1..free.len()).rev() {
            free.swap(i, (rng.next_u64() % (i as u64 + 1)) as usize);
        }
        for stack in stacks {
            let Some(slot) = free.pop() else { break };
            container.slots[slot] = Some(stack);
        }
        self.mark_chunk_modified(pos);
    }

    pub fn container_at(&self, pos: IVec3) -> Option<&Container> {
        let (c, lx, ly, lz) = self.data.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.container_at(lx, ly, lz)
    }

    pub fn container_at_mut(&mut self, pos: IVec3) -> Option<&mut Container> {
        let (c, lx, ly, lz) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.container_at_mut(lx, ly, lz)
    }

    pub fn ensure_container(&mut self, pos: IVec3, len: usize) -> bool {
        let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        match c.container_at_mut(lx, ly, lz) {
            Some(existing) => existing.ensure_len(len),
            None => c.insert_container(lx, ly, lz, Container::with_len(len)),
        }
        self.note_block_entity_change(pos);
        true
    }

    pub fn take_container(&mut self, pos: IVec3) -> Option<Container> {
        let (c, lx, ly, lz) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        let container = c.take_container(lx, ly, lz);
        if container.is_some() {
            self.note_block_entity_change(pos);
        }
        container
    }

    /// Forget the block-entity record at a broken block's cell — the furnace
    /// machine state, the one per-cell record the BLOCK WRITE does not clear
    /// itself. (Orientation state — a torch's mount, a chest/furnace front —
    /// lives in the unified cell-state store and dies with the block write
    /// via `clear_on_block_change`; sweeping it here would wipe state a
    /// different block at the cell still owns.) The
    /// container itself is NOT taken here: breaking scatters it via
    /// [`take_container`](Self::take_container) at the anchor.
    pub fn forget_block_entity_records(&mut self, pos: IVec3) {
        let anchor = self.container_anchor(pos);
        self.forget_block_draw(anchor);
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.take_furnace(lx, ly, lz);
            self.note_block_entity_change(pos);
        }
    }

    pub fn container_anchor(&self, pos: IVec3) -> IVec3 {
        self.model_group(pos)
            .map(|(_, base, _)| base)
            .unwrap_or(pos)
    }
}

#[cfg(test)]
mod tests;
