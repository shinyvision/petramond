//! Registry queries: name↔id resolution, tag membership, and item row
//! reads. Everything here is registry-only — legal on ANY instance (server,
//! worldgen, client), any time — and session-stable: resolve once in
//! [`Mod::init`] and keep the result in mod state, but NEVER persist numeric
//! ids (names are the stable identity).
//!
//! Items have ONE mod-facing identity, the registry NAME (`"petramond:coal"`,
//! `"farming:wheat"`); [`ItemId`]s in event payloads bridge to it through
//! [`resolve_item`] / [`item_names`].

use mod_api::{BlockId, ItemId, ItemInfoData, MobId};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt::host_fn;

host_fn! {
    /// Resolve a block registry name (`"petramond:stone"`, `"mymod:gadget"`) to its
    /// session-scoped runtime id. Works everywhere, worldgen instances included —
    /// resolve once in [`Mod::init`] and keep the id in mod state (but NEVER
    /// persist it: ids can change between sessions; names are the stable identity).
    pub fn resolve_block(name: &str) -> Option<BlockId> => ResolveBlock { name: name.into() } => Block
}

host_fn! {
    /// Resolve an item registry name to this session's numeric [`ItemId`], or
    /// `None` for an unknown name — the item twin of [`resolve_block`], same
    /// contract. Resolve once in `init` and compare against the ids in event
    /// payloads (`item_use_pre`); the reverse direction is [`item_names`].
    pub fn resolve_item(name: &str) -> Option<ItemId> => ResolveItem { name: name.into() } => Item
}

host_fn! {
    /// Resolve session block ids back to their registry names — the reverse of
    /// [`resolve_block`], batched (resolve a whole [`blocks_by_tag`] result in
    /// one crossing; at most 4096 ids per call — the sim batch cap, far past
    /// the 256-id space). Parallel to `blocks`; `None` = unregistered id.
    pub fn block_names(blocks: Vec<BlockId>) -> Vec<Option<String>>
        => BlockNames { blocks } => Names
}

host_fn! {
    /// Resolve session item ids back to their registry names — the reverse of
    /// [`resolve_item`], batched like [`block_names`]. How an id from an event
    /// payload or [`items_by_tag`] reaches the name-addressed calls
    /// ([`crate::give_item`], [`item_info`]).
    pub fn item_names(items: Vec<ItemId>) -> Vec<Option<String>>
        => ItemNames { items } => Names
}

host_fn! {
    /// Resolve a mob species key (`"petramond:sheep"` — the same string
    /// [`spawn_mob`](crate::spawn_mob) and `MobSnapshot::key` speak) to its
    /// session-scoped [`MobId`] — the mob twin of [`resolve_item`], same
    /// contract. Compare against the `kind` in `mob_died`/`mob_spawned`/
    /// `mob_damage_pre` payloads; the reverse direction is [`mob_names`].
    pub fn resolve_mob(key: &str) -> Option<MobId> => ResolveMob { key: key.into() } => MobKind
}

host_fn! {
    /// Resolve session mob species ids back to their keys — the reverse of
    /// [`resolve_mob`], batched like [`item_names`]. Parallel to `mobs`;
    /// `None` = unregistered id.
    pub fn mob_names(mobs: Vec<MobId>) -> Vec<Option<String>>
        => MobNames { mobs } => Names
}

host_fn! {
    /// Every registered block carrying `tag`, in id order — engine tags as
    /// `"petramond:<name>"` (e.g. `"petramond:leaves"`), pack tags as their
    /// `"mod_id:name"`. A name nothing lists is an empty set; a query never
    /// registers a tag. Tag-driven policy picks up pack-added blocks with no
    /// code change.
    pub fn blocks_by_tag(tag: &str) -> Vec<BlockId>
        => BlocksByTag { tag: tag.into() } => BlockList
}

host_fn! {
    /// Every registered item carrying `tag`, in id order — the item twin of
    /// [`blocks_by_tag`], same contract.
    pub fn items_by_tag(tag: &str) -> Vec<ItemId>
        => ItemsByTag { tag: tag.into() } => ItemList
}

host_fn! {
    /// One item's registry row by registry NAME (stack cap, fuel burn ticks,
    /// tags, display name, block link, tool, food, engine use key) — the same
    /// rows engine mechanics read. `None` = unknown name. Row data is
    /// session-stable — cache it mod-side instead of re-asking per tick.
    pub fn item_info(item: &str) -> Option<ItemInfoData> => ItemInfo { item: item.into() } => ItemInfo
}

/// [`resolve_block`] that also logs a "not registered" line on `None` — the
/// standard init-time shape: resolution failure is worth one log line, then
/// the mod degrades on the `None`.
pub fn resolve_block_logged(name: &str) -> Option<BlockId> {
    let id = resolve_block(name);
    if id.is_none() {
        crate::log(&format!("block '{name}' is not registered"));
    }
    id
}

/// [`resolve_item`] that also logs a "not registered" line on `None` — the
/// item twin of [`resolve_block_logged`].
pub fn resolve_item_logged(name: &str) -> Option<ItemId> {
    let id = resolve_item(name);
    if id.is_none() {
        crate::log(&format!("item '{name}' is not registered"));
    }
    id
}
