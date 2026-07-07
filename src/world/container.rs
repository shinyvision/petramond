//! Generic container block-entities at the world level: world-coordinate
//! access to the section-owned slot stores that back chests, furnaces, and
//! mod container blocks alike.
//!
//! Containers don't tick by themselves — a furnace's tick reads its container
//! through [`super::furnace`], and a mod block's meaning lives in its owning
//! mod — so these are thin world↔section coordinate wrappers for GUI edits,
//! mod host calls, and breaking.

use crate::container::Container;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The container at a world block position, if one is stored there.
    pub fn container_at(&self, pos: IVec3) -> Option<&Container> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.container_at(lx, ly, lz)
    }

    /// Mutable handle to the container at a world block position (GUI edits
    /// and mod `ContainerSet` writes).
    pub fn container_at_mut(&mut self, pos: IVec3) -> Option<&mut Container> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.container_at_mut(lx, ly, lz)
    }

    /// Make sure a container with at least `len` slots exists at `pos`
    /// (created empty, or grown — never shrunk — if a document re-authored
    /// with more slots). No-op if the owning chunk is not loaded. Returns
    /// whether a container is present afterwards.
    pub fn ensure_container(&mut self, pos: IVec3, len: usize) -> bool {
        let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        match c.container_at_mut(lx, ly, lz) {
            Some(existing) => existing.ensure_len(len),
            None => c.insert_container(lx, ly, lz, Container::with_len(len)),
        }
        self.note_block_entity_change(pos);
        true
    }

    /// Remove and return the container at a world position (block break),
    /// if any.
    pub fn take_container(&mut self, pos: IVec3) -> Option<Container> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        let container = c.take_container(lx, ly, lz);
        if container.is_some() {
            self.note_block_entity_change(pos);
        }
        container
    }

    /// Forget every sibling block-entity record at a broken block's cell —
    /// machine state (furnace), entity facing (chest/furnace front), torch
    /// orientation — in one unconditional sweep. The maps share the same cell
    /// key and clearing an absent record is free, so the break path needs no
    /// per-block ladder (and the next facing-bearing block can't be
    /// forgotten). The container itself is NOT taken here: breaking scatters
    /// it via [`take_container`](Self::take_container) at the anchor.
    pub fn forget_block_entity_records(&mut self, pos: IVec3) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.take_furnace(lx, ly, lz);
            c.take_entity_facing(lx, ly, lz);
            c.take_torch(lx, ly, lz);
            self.note_block_entity_change(pos);
        }
    }

    /// The canonical container position for the block at `pos`: multi-cell
    /// model blocks share ONE container keyed at the model group's base cell,
    /// so opening the same placed object from any of its cells edits the same
    /// slots; everything else keys at its own cell.
    pub fn container_anchor(&self, pos: IVec3) -> IVec3 {
        self.model_group(pos)
            .map(|(_, base, _)| base)
            .unwrap_or(pos)
    }
}
