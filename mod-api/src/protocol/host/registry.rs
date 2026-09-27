use crate::data::BlockRecord;
use crate::ids::{BlockId, ConditionId, ItemId, MobId};
use crate::legality::prelude::*;

host_domain! {
    RegistryCall {
        ResolveBlock {
            name: String,
        } => legal(EVERY, Any, Read),
        ItemInfo {
            item: String,
            data: Vec<(String, Vec<u8>)>,
        } => legal(EVERY, Any, Read),
        ResolveItem {
            name: String,
        } => legal(EVERY, Any, Read),
        BlocksByTag {
            tag: String,
        } => legal(EVERY, Any, Read),
        ItemsByTag {
            tag: String,
        } => legal(EVERY, Any, Read),
        BlockNames {
            blocks: Vec<BlockId>,
        } => legal(EVERY, Any, Read),
        ItemNames {
            items: Vec<ItemId>,
        } => legal(EVERY, Any, Read),
        ResolveMob {
            key: String,
        } => legal(EVERY, Any, Read),
        MobNames {
            mobs: Vec<MobId>,
        } => legal(EVERY, Any, Read),
        ResolveShape {
            key: String,
        } => legal(EVERY, Any, Read),
        ItemDataGet {
            item: ItemId,
            key: String,
        } => legal(EVERY, Any, Read),
        ItemsWithData {
            key: String,
        } => legal(EVERY, Any, Read),
        BlockDataGet {
            block: BlockId,
            key: String,
        } => legal(EVERY, Any, Read),
        BlocksWithData {
            key: String,
        } => legal(EVERY, Any, Read),
        BlockInfo {
            block: BlockId,
        } => legal(EVERY, Any, Read),
        StructureInfo {
            key: String,
        } => legal(EVERY, Any, Read),
        LootRoll {
            key: String,
            seed: u64,
        } => legal(EVERY, Any, Read),
        MobDataGet {
            mob: crate::MobId,
            key: String,
        } => legal(EVERY, Any, Read),
        MobsWithData {
            key: String,
        } => legal(EVERY, Any, Read),
        ResolveCondition {
            key: String,
        } => legal(EVERY, Any, Read),
        ConditionNames {
            conditions: Vec<ConditionId>,
        } => legal(EVERY, Any, Read),
        BlockInfos {
            blocks: Vec<BlockId>,
        } => legal(EVERY, Any, Read),
        BlockRecordPlans {
            records: Vec<BlockRecord>,
        } => legal(EVERY, Any, Read),
    }
}
