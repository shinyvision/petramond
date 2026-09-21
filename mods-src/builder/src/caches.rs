//! Session-stable registry answers the golem and the table read repeatedly.

use crate::fx::HashMap;

use mod_sdk::*;

#[derive(Default)]
pub struct Caches {
    items: HashMap<String, Option<ItemInfoData>>,
    blocks: HashMap<BlockId, Option<BlockInfoData>>,
    named: HashMap<String, Option<BlockId>>,
}

impl Caches {
    pub fn item(&mut self, name: &str) -> Option<&ItemInfoData> {
        self.items
            .entry(name.to_owned())
            .or_insert_with(|| item_info(name).map(|b| *b))
            .as_ref()
    }

    pub fn block(&mut self, block: BlockId) -> Option<&BlockInfoData> {
        self.blocks
            .entry(block)
            .or_insert_with(|| block_info(block).map(|b| *b))
            .as_ref()
    }

    /// A block row's facts by its registry name.
    pub fn block_named(&mut self, name: &str) -> Option<&BlockInfoData> {
        let id = *self
            .named
            .entry(name.to_owned())
            .or_insert_with(|| resolve_block(name));
        self.block(id?)
    }

    pub fn max_stack(&mut self, name: &str) -> u8 {
        self.item(name).map_or(64, |i| i.max_stack.max(1))
    }

    pub fn display_name(&mut self, name: &str) -> String {
        self.item(name)
            .map_or_else(|| name.to_owned(), |i| i.display_name.clone())
    }
}
