use mod_api::{BlockId, ItemId, ItemInfoData, ItemStackData, MobId};

host_fn! {
    pub fn mob_data(mob: MobId, key: &str) -> Option<Vec<u8>>
        => MobDataGet { mob, key: key.into() } => Bytes
}

host_fn! {
    pub fn mobs_with_data(key: &str) -> Vec<(MobId, String)>
        => MobsWithData { key: key.into() } => MobDataRows
}

#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::host_fn;

host_fn! {
    pub fn resolve_block(name: &str) -> Option<BlockId> => ResolveBlock { name: name.into() } => Block
}

host_fn! {
    pub fn resolve_item(name: &str) -> Option<ItemId> => ResolveItem { name: name.into() } => Item
}

host_fn! {
    pub fn block_names(blocks: Vec<BlockId>) -> Vec<Option<String>>
        => BlockNames { blocks } => Names
}

host_fn! {
    pub fn item_names(items: Vec<ItemId>) -> Vec<Option<String>>
        => ItemNames { items } => Names
}

host_fn! {
    pub fn resolve_shape(key: &str) -> Option<u16> => ResolveShape { key: key.into() } => MaybeU16
}

host_fn! {
    pub fn resolve_mob(key: &str) -> Option<MobId> => ResolveMob { key: key.into() } => MobKind
}

host_fn! {
    pub fn mob_names(mobs: Vec<MobId>) -> Vec<Option<String>>
        => MobNames { mobs } => Names
}

host_fn! {
    pub fn resolve_condition(key: &str) -> Option<mod_api::ConditionInfoData>
        => ResolveCondition { key: key.into() } => Condition
}

host_fn! {
    pub fn condition_names(conditions: Vec<mod_api::ConditionId>) -> Vec<Option<String>>
        => ConditionNames { conditions } => Names
}

host_fn! {
    pub fn blocks_by_tag(tag: &str) -> Vec<BlockId>
        => BlocksByTag { tag: tag.into() } => BlockList
}

host_fn! {
    pub fn items_by_tag(tag: &str) -> Vec<ItemId>
        => ItemsByTag { tag: tag.into() } => ItemList
}

host_fn! {
    pub fn item_info(item: &str) -> Option<Box<ItemInfoData>>
        => ItemInfo { item: item.into(), data: Vec::new() } => ItemInfo
}

host_fn! {
    pub fn stack_info(stack: &ItemStackData) -> Option<Box<ItemInfoData>>
        => ItemInfo { item: stack.item.clone(), data: stack.data.clone() } => ItemInfo
}

pub fn resolve_block_logged(name: &str) -> Option<BlockId> {
    let id = resolve_block(name);
    if id.is_none() {
        crate::log(&format!("block '{name}' is not registered"));
    }
    id
}

pub fn resolve_mob_logged(name: &str) -> Option<MobId> {
    let id = resolve_mob(name);
    if id.is_none() {
        crate::log(&format!("mob '{name}' is not registered"));
    }
    id
}

pub fn resolve_condition_logged(key: &str) -> Option<mod_api::ConditionInfoData> {
    let info = resolve_condition(key);
    if info.is_none() {
        crate::log(&format!("condition '{key}' is not registered"));
    }
    info
}

pub fn resolve_item_logged(name: &str) -> Option<ItemId> {
    let id = resolve_item(name);
    if id.is_none() {
        crate::log(&format!("item '{name}' is not registered"));
    }
    id
}

host_fn! {
    pub fn item_data(item: ItemId, key: &str) -> Option<Vec<u8>>
        => ItemDataGet { item, key: key.into() } => Bytes
}

host_fn! {
    pub fn items_with_data(key: &str) -> Vec<(ItemId, String)>
        => ItemsWithData { key: key.into() } => ItemDataRows
}

host_fn! {
    pub fn block_data(block: BlockId, key: &str) -> Option<Vec<u8>>
        => BlockDataGet { block, key: key.into() } => Bytes
}

host_fn! {
    pub fn blocks_with_data(key: &str) -> Vec<(BlockId, String)>
        => BlocksWithData { key: key.into() } => BlockDataRows
}

host_fn! {
    pub fn block_info(block: BlockId) -> Option<Box<mod_api::BlockInfoData>>
        => BlockInfo { block } => BlockInfo
}

host_fn! {
    pub fn block_infos(blocks: Vec<BlockId>) -> Vec<Option<mod_api::BlockInfoData>>
        => BlockInfos { blocks } => BlockInfos
}

pub fn registered_blocks() -> Vec<(BlockId, mod_api::BlockInfoData)> {
    dense_prefix(block_infos)
}

fn dense_prefix<T>(mut query: impl FnMut(Vec<BlockId>) -> Vec<Option<T>>) -> Vec<(BlockId, T)> {
    const PAGE: u16 = 256;
    let mut out = Vec::new();
    let mut first = 0u16;
    loop {
        let end = first.saturating_add(PAGE);
        let asked = usize::from(end - first);
        let infos = query((first..end).map(BlockId).collect());
        let answered = infos.len();
        for (id, info) in (first..).zip(infos) {
            match info {
                Some(info) => out.push((BlockId(id), info)),
                None => return out,
            }
        }
        if answered < asked || end == u16::MAX {
            return out;
        }
        first = end;
    }
}

host_fn! {
    pub fn block_record_plans(records: Vec<mod_api::BlockRecord>) -> Vec<mod_api::RecordPlan>
        => BlockRecordPlans { records } => RecordPlans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_read_in_pages_up_to_the_first_unregistered_id() {
        let mut pages = Vec::new();
        let found = dense_prefix(|ids: Vec<BlockId>| {
            pages.push(ids.len());
            ids.into_iter()
                .map(|id| (id.0 < 600).then_some(id.0 * 2))
                .collect()
        });
        assert_eq!(
            pages,
            [256, 256, 256],
            "one page per crossing, none past the gap"
        );
        assert_eq!(found.len(), 600);
        assert_eq!(found[599], (BlockId(599), 1198));
    }

    #[test]
    fn a_short_reply_ends_the_read() {
        let mut calls = 0;
        let found = dense_prefix(|ids: Vec<BlockId>| {
            calls += 1;
            ids.into_iter().take(10).map(|id| Some(id.0)).collect()
        });
        assert_eq!(calls, 1);
        assert_eq!(found.len(), 10);
    }
}
