use std::collections::HashMap;

use crate::chunk::section_idx;
use crate::container::Container;
use crate::facing::Facing;
use crate::furnace::Furnace;
use crate::item::{ItemStack, ItemType};

use super::{BlockEntities, Section};

impl Section {
    // --- Block-entity maps ------------------------------------------------------

    /// Section-local block-index key for a block-entity map (`section_idx` fits a
    /// `u16`).
    #[inline]
    fn block_entity_key(x: usize, y: usize, z: usize) -> u16 {
        section_idx(x, y, z) as u16
    }

    /// Invert [`block_entity_key`](Self::block_entity_key): `x = key & 15`,
    /// `y = key >> 8`, `z = (key >> 4) & 15`.
    #[inline]
    fn block_entity_coords(key: u16) -> (usize, usize, usize) {
        (
            (key & 0x000F) as usize,
            (key >> 8) as usize,
            ((key >> 4) & 0x000F) as usize,
        )
    }

    #[inline]
    fn entities_mut(&mut self) -> &mut BlockEntities {
        self.entities.get_or_insert_default()
    }

    #[inline]
    pub fn furnace_at(&self, x: usize, y: usize, z: usize) -> Option<&Furnace> {
        self.entities
            .as_deref()
            .and_then(|e| e.furnaces.get(&Self::block_entity_key(x, y, z)))
    }

    #[inline]
    pub fn furnace_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Furnace> {
        self.entities
            .as_deref_mut()
            .and_then(|e| e.furnaces.get_mut(&Self::block_entity_key(x, y, z)))
    }

    pub fn insert_furnace(&mut self, x: usize, y: usize, z: usize, furnace: Furnace) {
        self.entities_mut()
            .furnaces
            .insert(Self::block_entity_key(x, y, z), furnace);
        self.modified = true;
    }

    pub fn take_furnace(&mut self, x: usize, y: usize, z: usize) -> Option<Furnace> {
        let removed = self
            .entities
            .as_deref_mut()
            .and_then(|e| e.furnaces.remove(&Self::block_entity_key(x, y, z)));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    /// The furnace state and its container slots at one cell, split-borrowed
    /// (they live in sibling maps under the same key).
    pub fn furnace_parts_mut(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Option<(&mut Furnace, &mut Container)> {
        let key = Self::block_entity_key(x, y, z);
        let e = self.entities.as_deref_mut()?;
        let furnace = e.furnaces.get_mut(&key)?;
        let container = e.containers.get_mut(&key)?;
        Some((furnace, container))
    }

    #[inline]
    pub fn is_furnace_lit(&self, x: usize, y: usize, z: usize) -> bool {
        self.furnace_at(x, y, z).is_some_and(Furnace::is_lit)
    }

    #[inline]
    pub fn furnaces(&self) -> &HashMap<u16, Furnace> {
        match &self.entities {
            Some(e) => &e.furnaces,
            None => crate::block_state::empty_map!(Furnace),
        }
    }

    /// Which way the facing block-entity (chest, furnace) at a cell points.
    #[inline]
    pub fn entity_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.entity_facings()
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    pub fn insert_entity_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.entities_mut()
            .entity_facings
            .insert(Self::block_entity_key(x, y, z), facing);
        self.modified = true;
    }

    pub fn take_entity_facing(&mut self, x: usize, y: usize, z: usize) {
        if self
            .entities
            .as_deref_mut()
            .and_then(|e| e.entity_facings.remove(&Self::block_entity_key(x, y, z)))
            .is_some()
        {
            self.modified = true;
        }
    }

    #[inline]
    pub fn entity_facings(&self) -> &HashMap<u16, Facing> {
        match &self.entities {
            Some(e) => &e.entity_facings,
            None => crate::block_state::empty_map!(Facing),
        }
    }

    #[inline]
    pub fn container_at(&self, x: usize, y: usize, z: usize) -> Option<&Container> {
        self.entities
            .as_deref()
            .and_then(|e| e.containers.get(&Self::block_entity_key(x, y, z)))
    }

    #[inline]
    pub fn container_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Container> {
        self.entities
            .as_deref_mut()
            .and_then(|e| e.containers.get_mut(&Self::block_entity_key(x, y, z)))
    }

    pub fn insert_container(&mut self, x: usize, y: usize, z: usize, c: Container) {
        self.entities_mut()
            .containers
            .insert(Self::block_entity_key(x, y, z), c);
        self.modified = true;
    }

    pub fn take_container(&mut self, x: usize, y: usize, z: usize) -> Option<Container> {
        let removed = self
            .entities
            .as_deref_mut()
            .and_then(|e| e.containers.remove(&Self::block_entity_key(x, y, z)));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    #[inline]
    pub fn containers(&self) -> &HashMap<u16, Container> {
        match &self.entities {
            Some(e) => &e.containers,
            None => crate::block_state::empty_map!(Container),
        }
    }

    /// Advance every furnace one game tick. Returns the local coordinates of
    /// furnaces whose lit texture changed (so the world can enqueue mesh/block
    /// updates). No-op for the common furnace-free section.
    pub fn tick_furnaces(
        &mut self,
        smelt: impl Fn(ItemType) -> Option<ItemStack>,
    ) -> Vec<(usize, usize, usize)> {
        let Some(entities) = self.entities.as_deref_mut() else {
            return Vec::new();
        };
        if entities.furnaces.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        let mut relit = Vec::new();
        // Key order, not map order: `relit` feeds block-update scheduling, and
        // deterministic ticks (the multiplayer contract) forbid HashMap
        // iteration order leaking into it.
        let mut keys: Vec<u16> = entities.furnaces.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let f = entities.furnaces.get_mut(&key).expect("key just listed");
            // The furnace's slots live in the shared container map under the
            // same key (sibling field — disjoint borrow).
            let Some(container) = entities.containers.get_mut(&key) else {
                continue;
            };
            let was_lit = f.is_lit();
            if f.tick(&mut container.slots, &smelt) {
                changed = true;
            }
            if f.is_lit() != was_lit {
                relit.push(Self::block_entity_coords(key));
            }
        }
        if changed {
            self.modified = true;
        }
        if !relit.is_empty() {
            self.dirty = true;
        }
        relit
    }
}
