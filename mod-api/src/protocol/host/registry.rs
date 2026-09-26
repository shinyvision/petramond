//! The process-wide registries: name/id resolution both ways, tag membership,
//! rows and instance data, structures, loot, conditions, record plans. Legal on
//! every instance at any time.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::BlockRecord;
use crate::ids::{BlockId, ConditionId, ItemId, MobId};
use crate::legality::prelude::*;

host_domain! {
    /// The process-wide registries: name/id resolution both ways, tag membership,
    /// rows and instance data, structures, loot, conditions, record plans. Legal on
    /// every instance at any time.
    RegistryCall {
        /// Resolve a block registry NAME (`"petramond:stone"`, `"kitchen:oven"`) to
        /// its session-scoped runtime id. Registry-only, needs no simulation
        /// context — legal on ANY instance, any time (worldgen and client
        /// instances included). `None` = not registered (a typo'd or absent pack —
        /// degrade gracefully, don't panic). → [`HostRet::Block`](crate::HostRet::Block).
        ResolveBlock {
            name: String,
        } => legal(EVERY, Any, Read),
        /// Read one item's registry row (by registry NAME): the same
        /// [`ItemInfoData`](crate::ItemInfoData) fields engine mechanics read, so mod logic (a
        /// fuel-fired oven, a filtering hopper, a tool gate) composes with
        /// pack-added items for free. `data` is a stack's instance data (empty
        /// = the bare row): the answer is the row AS THAT STACK CARRIES IT,
        /// every instance override the engine itself honours applied (an
        /// augmented tool's `petramond:tool` override lands in `tool`) — what a
        /// mod reading a HELD tool's damage or speed must ask for, since the
        /// bare row silently ignores augments. Registry-only like
        /// [`RegistryCall::ResolveItem`]: legal on any instance, any time; row data
        /// is session-stable — cache the bare row mod-side. `None` = unknown
        /// name. → [`HostRet::ItemInfo`](crate::HostRet::ItemInfo).
        ItemInfo {
            item: String,
            data: Vec<(String, Vec<u8>)>,
        } => legal(EVERY, Any, Read),
        /// Resolve an item registry NAME to this session's numeric id, or `None`
        /// for an unknown name. Registry-only (no world access): legal on any
        /// instance, any time — the [`RegistryCall::ResolveBlock`] contract. This is
        /// how a mod identifies its own items in id-bearing event payloads
        /// (e.g. `item_use_pre`) without persisting numeric ids. The reverse
        /// direction is [`RegistryCall::ItemNames`].
        /// → [`HostRet::Item`](crate::HostRet::Item).
        ResolveItem {
            name: String,
        } => legal(EVERY, Any, Read),
        /// Every registered block carrying `tag`, in id order. Registry-only
        /// like [`RegistryCall::ResolveBlock`] — legal on any instance, any time.
        /// Engine tags read as `petramond:<name>` (e.g. `petramond:leaves`);
        /// pack tags as their `mod_id:name`. A name nothing lists is simply an
        /// empty set, never an error — querying cannot register a tag.
        /// → [`HostRet::BlockList`](crate::HostRet::BlockList).
        BlocksByTag {
            tag: String,
        } => legal(EVERY, Any, Read),
        /// Every registered item carrying `tag`, in id order — the item twin of
        /// [`RegistryCall::BlocksByTag`], same contract: registry-only (legal on any
        /// instance, any time), engine tags as `petramond:<name>`, pack tags as
        /// their `mod_id:name`, and a name nothing lists is simply an empty set —
        /// querying cannot register a tag. → [`HostRet::ItemList`](crate::HostRet::ItemList).
        ItemsByTag {
            tag: String,
        } => legal(EVERY, Any, Read),
        /// Resolve session block ids back to their registry NAMES — the reverse of
        /// [`RegistryCall::ResolveBlock`], batched at the message level (resolve a
        /// whole [`RegistryCall::BlocksByTag`] result in one crossing). Reply parallel
        /// to `blocks`; `None` = unregistered id. At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) ids per call — a legitimate
        /// batch never approaches it. Registry-only: legal on any instance, any
        /// time. → [`HostRet::Names`](crate::HostRet::Names).
        BlockNames {
            blocks: Vec<BlockId>,
        } => legal(EVERY, Any, Read),
        /// Resolve session item ids back to their registry NAMES — the reverse of
        /// [`RegistryCall::ResolveItem`], same batching and contract as
        /// [`RegistryCall::BlockNames`]. How an id from an event payload or
        /// [`RegistryCall::ItemsByTag`] reaches the name-addressed calls
        /// ([`PlayerCall::GiveItem`](crate::PlayerCall::GiveItem), [`RegistryCall::ItemInfo`]). → [`HostRet::Names`](crate::HostRet::Names).
        ItemNames {
            items: Vec<ItemId>,
        } => legal(EVERY, Any, Read),
        /// Resolve a mob species key (`"petramond:sheep"`, `"monsters:zombie"` —
        /// the `key` field of a `mobs.json` row, the same string
        /// [`EntityCall::SpawnMob`](crate::EntityCall::SpawnMob) speaks; a [`MobSnapshot`](crate::MobSnapshot) deliberately carries
        /// only the numeric `kind`, which is what this resolves TO) to its
        /// session-scoped [`MobId`] — how a mod filters the `kind` in
        /// `mob_died`/`mob_spawned`/`mob_damage_pre` payloads without string
        /// round-trips. Registry-only like [`RegistryCall::ResolveBlock`]: legal on
        /// any instance, any time. `None` = unregistered key. →
        /// [`HostRet::MobKind`](crate::HostRet::MobKind).
        ResolveMob {
            key: String,
        } => legal(EVERY, Any, Read),
        /// Resolve session mob species ids back to their keys — the reverse of
        /// [`RegistryCall::ResolveMob`], batched like [`RegistryCall::ItemNames`]. Reply
        /// parallel to `mobs`; `None` = unregistered id. Registry-only: legal on
        /// any instance, any time. → [`HostRet::Names`](crate::HostRet::Names).
        MobNames {
            mobs: Vec<MobId>,
        } => legal(EVERY, Any, Read),
        /// Resolve a block SHAPE-KIND registry key (`"petramond:fence"`,
        /// `"mymod:gate"`) to its session-local numeric id — the shape twin of
        /// [`RegistryCall::ResolveBlock`], for a custom-shape mod branching on the
        /// `shape_kind` its bake calls carry. Registry-only (legal on any instance).
        /// `None` = no such shape kind. → [`HostRet::MaybeByte`](crate::HostRet::MaybeByte).
        ResolveShape {
            key: String,
        } => legal(EVERY, Any, Read),
        /// The item row's consumer-data entry `key` as raw JSON text — the item
        /// INTEROP surface: a row's `data` map holds namespaced entries written
        /// in a CONSUMING system's vocabulary (any pack may attach any
        /// consumer's key to its own rows, or to existing rows via catalog
        /// `{"patch", "data"}` rows; the engine's own `petramond:fuel` /
        /// `petramond:tool` ride the same surface). The value is opaque —
        /// the consumer parses what it understands and ignores the rest.
        /// Registry-only (legal on any instance, any time). `Bytes(None)` =
        /// no such entry. → [`HostRet::Bytes`](crate::HostRet::Bytes).
        ItemDataGet {
            item: ItemId,
            key: String,
        } => legal(EVERY, Any, Read),
        /// Every registered item carrying data entry `key`, WITH each row's raw
        /// JSON value, in id order — the enumeration a consumer runs once at
        /// init to build its table (one crossing, like every batch call).
        /// Registry-only. → [`HostRet::ItemDataRows`](crate::HostRet::ItemDataRows).
        ItemsWithData {
            key: String,
        } => legal(EVERY, Any, Read),
        /// The block twin of [`RegistryCall::ItemDataGet`]. Registry-only.
        /// → [`HostRet::Bytes`](crate::HostRet::Bytes).
        BlockDataGet {
            block: BlockId,
            key: String,
        } => legal(EVERY, Any, Read),
        /// The block twin of [`RegistryCall::ItemsWithData`]. Registry-only.
        /// → [`HostRet::BlockDataRows`](crate::HostRet::BlockDataRows).
        BlocksWithData {
            key: String,
        } => legal(EVERY, Any, Read),
        /// The block twin of [`ItemInfo`](Self::ItemInfo): the row's stable
        /// harvest facts ([`BlockInfoData`](crate::BlockInfoData) — material,
        /// hardness, harvest tier, the tool family the gate credits, and the
        /// item that places the block). The engine's own material→tool ladder
        /// answers here so a mod judging a break never re-derives it (the
        /// duplicated-constants trap). Registry-only (legal on any instance,
        /// any time). `None` = unregistered id. → [`HostRet::BlockInfo`](crate::HostRet::BlockInfo).
        BlockInfo {
            block: BlockId,
        } => legal(EVERY, Any, Read),
        /// Compiled template metadata. Registry-only, legal on every runtime side.
        /// Resolve once during initialization. → [`HostRet::StructureInfo`](crate::HostRet::StructureInfo).
        StructureInfo {
            key: String,
        } => legal(EVERY, Any, Read),
        /// Sample a reward table without delivering it. Equal table/seed pairs
        /// return equal results; an unknown table returns None. At most 256 entries.
        LootRoll {
            key: String,
            seed: u64,
        } => legal(EVERY, Any, Read),
        /// A species' immutable consumer metadata. Registry-only; absent returns `Bytes(None)`.
        MobDataGet {
            mob: crate::MobId,
            key: String,
        } => legal(EVERY, Any, Read),
        /// Species carrying this consumer key, with JSON values in registry order.
        MobsWithData {
            key: String,
        } => legal(EVERY, Any, Read),
        /// Resolve a `conditions.json` key to its row → [`HostRet::Condition`](crate::HostRet::Condition)
        /// (`None` = unregistered). Registry-only, legal on any instance.
        ResolveCondition {
            key: String,
        } => legal(EVERY, Any, Read),
        /// Condition ids back to their keys, parallel to `conditions` →
        /// [`HostRet::Names`](crate::HostRet::Names).
        ConditionNames {
            conditions: Vec<ConditionId>,
        } => legal(EVERY, Any, Read),
        /// [`RegistryCall::BlockInfo`] for many ids in one crossing, parallel to
        /// `blocks` (`None` = unregistered id; at most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) ids). How a consumer classifies
        /// the whole block registry once at init. Registry-only, legal on any
        /// instance. → [`HostRet::BlockInfos`](crate::HostRet::BlockInfos).
        BlockInfos {
            blocks: Vec<BlockId>,
        } => legal(EVERY, Any, Read),
        /// What each record asks of construction ([`RecordPlan`]), parallel to
        /// `records`. Registry-only: legal on any instance. At most
        /// `SIM_BATCH_MAX` records. → [`HostRet::RecordPlans`](crate::HostRet::RecordPlans).
        BlockRecordPlans {
            records: Vec<BlockRecord>,
        } => legal(EVERY, Any, Read),
    }
}
