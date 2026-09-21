use petramond_world::{
    block::{Block, ShapeState, SHAPE_STATE_MAX},
    container::Container,
    furnace::Furnace,
    item::{variant, ItemStack, ItemType, VariantId},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SavedStack {
    pub item: String,
    pub count: u8,
    pub data: Vec<u8>,
}

impl SavedStack {
    pub fn capture(stack: ItemStack) -> Self {
        Self {
            item: stack.item.registry_name().into(),
            count: stack.count,
            data: variant::blob(stack.variant)
                .map(|b| b.as_ref().clone())
                .unwrap_or_default(),
        }
    }

    fn resolve(&self) -> Result<ItemStack, String> {
        let item =
            ItemType::by_name(&self.item).ok_or_else(|| format!("Missing item: {}", self.item))?;
        if self.count == 0 || self.count > item.max_stack_size() {
            return Err(format!("Invalid stack: {}", self.item));
        }
        let variant = if self.data.is_empty() {
            VariantId::NONE
        } else {
            variant::intern_blob(&self.data).ok_or("Invalid or exhausted item data")?
        };
        Ok(ItemStack::with_variant(item, self.count, variant))
    }
}

/// Persistent cell data. Registry references are names, including references inside shape bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellData {
    pub block: String,
    pub state: Vec<u8>,
    pub state_ids: BTreeMap<u8, String>,
    pub fluid: u8,
    pub kv: BTreeMap<String, Vec<u8>>,
    pub container: Option<Vec<Option<SavedStack>>>,
    pub furnace: Option<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedCell {
    pub block: Block,
    pub state: ShapeState,
    pub fluid: u8,
    pub kv: BTreeMap<String, Vec<u8>>,
    pub container: Option<Container>,
    pub furnace: Option<Furnace>,
}

impl CellData {
    pub fn capture(cell: &ResolvedCell) -> Self {
        let mut state = cell.state.bytes().to_vec();
        let mut state_ids = BTreeMap::new();
        for i in 0..state.len() {
            if cell.state.id_mask() & (1 << i) != 0 {
                state_ids.insert(i as u8, block_name(Block::from_id(cell.state.id_at(i))));
                state[i] = 0;
                state[i + 1] = 0;
            }
        }
        Self {
            block: block_name(cell.block),
            state,
            state_ids,
            fluid: cell.fluid,
            kv: cell.kv.clone(),
            container: cell
                .container
                .as_ref()
                .map(|c| c.slots.iter().map(|s| s.map(SavedStack::capture)).collect()),
            furnace: cell
                .furnace
                .map(|f| [f.cook_progress, f.burn_remaining, f.burn_max]),
        }
    }

    pub fn validate(&self) -> Result<usize, String> {
        if self.block.len() > 256 || self.state.len() > SHAPE_STATE_MAX || self.kv.len() > 256 {
            return Err("Invalid cell data".into());
        }
        let mut used = 0u16;
        for (&i, name) in &self.state_ids {
            if usize::from(i) + 1 >= self.state.len() || name.len() > 256 || used & (3 << i) != 0 {
                return Err("Invalid shape references".into());
            }
            used |= 3 << i;
        }
        let mut bytes = 64
            + self.block.len()
            + self.state.len()
            + self.state_ids.values().map(String::len).sum::<usize>();
        for (key, value) in &self.kv {
            if key.len() > 256 || !key.contains(':') || value.len() > 65_536 {
                return Err("Invalid block instance data".into());
            }
            bytes += key.len() + value.len();
        }
        if let Some(slots) = &self.container {
            if slots.len() > petramond_world::container::MAX_CONTAINER_SLOTS {
                return Err("Container has too many slots".into());
            }
            for stack in slots.iter().flatten() {
                if stack.item.len() > 256
                    || stack.data.len() > 1024
                    || (!stack.data.is_empty() && variant::decode(&stack.data).is_none())
                {
                    return Err("Invalid stored item data".into());
                }
                bytes += 16 + stack.item.len() + stack.data.len();
            }
        }
        Ok(bytes)
    }

    pub fn resolve(&self) -> Result<ResolvedCell, String> {
        self.validate()?;
        let mut bytes = self.state.clone();
        let mut mask = 0;
        for (&i, name) in &self.state_ids {
            let id = resolve_block(name)?.id().to_le_bytes();
            bytes[usize::from(i)..usize::from(i) + 2].copy_from_slice(&id);
            mask |= 1 << i;
        }
        Ok(ResolvedCell {
            block: resolve_block(&self.block)?,
            state: ShapeState::with_ids(&bytes, mask),
            fluid: self.fluid,
            kv: self.kv.clone(),
            container: self
                .container
                .as_ref()
                .map(|slots| {
                    slots
                        .iter()
                        .map(|s| s.as_ref().map(SavedStack::resolve).transpose())
                        .collect::<Result<Vec<_>, _>>()
                        .map(|slots| Container { slots })
                })
                .transpose()?,
            furnace: self
                .furnace
                .map(|[cook_progress, burn_remaining, burn_max]| Furnace {
                    cook_progress,
                    burn_remaining,
                    burn_max,
                }),
        })
    }
}

impl CellData {
    /// The survival construction record of this cell: its row and state, and
    /// only the data an item carries — never stored inventories, machine
    /// state or other private cell data.
    pub fn construction_record(&self) -> Result<petramond_world::construction::Record, String> {
        let bare = CellData {
            block: self.block.clone(),
            state: self.state.clone(),
            state_ids: self.state_ids.clone(),
            fluid: 0,
            kv: self.kv.clone(),
            container: None,
            furnace: None,
        };
        let resolved = bare.resolve()?;
        Ok(petramond_world::construction::Record::portable(
            resolved.block,
            resolved.state,
            &resolved.kv,
        ))
    }

    /// A construction record in names, as it crosses a boundary.
    pub fn of_record(record: &petramond_world::construction::Record) -> Self {
        Self::capture(&ResolvedCell {
            block: record.block,
            state: record.state,
            fluid: 0,
            kv: record.data.clone(),
            container: None,
            furnace: None,
        })
    }
}

pub fn block_name(block: Block) -> String {
    petramond_world::registry::names()
        .blocks
        .name(block.id())
        .expect("registered block")
        .into()
}

pub fn resolve_block(name: &str) -> Result<Block, String> {
    petramond_world::registry::names()
        .blocks
        .id(name)
        .map(Block::from_id)
        .ok_or_else(|| format!("Missing block: {name}"))
}
