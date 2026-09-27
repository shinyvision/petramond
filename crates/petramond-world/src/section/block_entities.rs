use crate::chunk::section_idx;
use crate::container::Container;
use crate::furnace::Furnace;
use crate::item::{ItemStack, ItemType};

use super::{BlockEntities, CellMap, Section};

impl Section {
    #[inline]
    fn block_entity_key(x: usize, y: usize, z: usize) -> u16 {
        section_idx(x, y, z) as u16
    }

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
    pub fn furnaces(&self) -> &CellMap<Furnace> {
        match &self.entities {
            Some(e) => &e.furnaces,
            None => crate::block_state::empty_map!(Furnace),
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
    pub fn containers(&self) -> &CellMap<Container> {
        match &self.entities {
            Some(e) => &e.containers,
            None => crate::block_state::empty_map!(Container),
        }
    }

    pub fn tick_furnaces(
        &mut self,
        smelt: impl Fn(ItemType) -> Option<ItemStack>,
    ) -> Vec<(usize, usize, usize, crate::block::Block)> {
        use crate::block::Block;
        let Some(entities) = self.entities.as_deref_mut() else {
            return Vec::new();
        };
        if entities.furnaces.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        let mut reskin = Vec::new();
        for (&key, f) in entities.furnaces.iter_mut() {
            let Some(container) = entities.containers.get_mut(&key) else {
                continue;
            };
            if f.tick(&mut container.slots, &smelt) {
                changed = true;
            }
            let desired = if f.is_lit() {
                Block::FurnaceLit
            } else {
                Block::Furnace
            };
            let current = self.blocks.get(key as usize);
            if (current == Block::Furnace.id() || current == Block::FurnaceLit.id())
                && current != desired.id()
            {
                let (x, y, z) = Self::block_entity_coords(key);
                reskin.push((x, y, z, desired));
            }
        }
        if changed {
            self.modified = true;
        }
        reskin
    }
}
