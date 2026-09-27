use crate::fx::HashMap;

use crate::host::prelude::*;

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
            .or_insert_with(|| item_info(name))
            .as_ref()
    }

    pub fn block(&mut self, block: BlockId) -> Option<&BlockInfoData> {
        self.blocks
            .entry(block)
            .or_insert_with(|| block_info(block))
            .as_ref()
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::rows::STONE;
    use crate::testing::Session;

    #[test]
    fn registry_answers_are_asked_once_a_session() {
        let session = Session::flat(1);
        let mut caches = Caches::default();
        assert_eq!(caches.max_stack(crate::keys::BLUEPRINT), 1);
        assert_eq!(caches.max_stack("petramond:stone"), 64);
        assert_eq!(
            caches.max_stack("nobody:nothing"),
            64,
            "unknown: as most stack"
        );
        assert_eq!(caches.display_name("petramond:oak_planks"), "Oak Planks");
        assert_eq!(caches.display_name("nobody:nothing"), "nobody:nothing");
        assert_eq!(caches.block(STONE).map(|b| b.hardness), Some(1.5));
        assert_eq!(
            caches.block_named("petramond:stone").map(|b| b.hardness),
            Some(1.5)
        );
        assert!(caches.block_named("nobody:nothing").is_none());
        drop(session);
        assert_eq!(caches.display_name("petramond:oak_planks"), "Oak Planks");
        assert_eq!(caches.block(STONE).map(|b| b.hardness), Some(1.5));
        assert!(caches.block_named("nobody:nothing").is_none());
    }
}
