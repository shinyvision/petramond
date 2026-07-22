//! Client cache of Layer-3 custom shapes' baked ITEM geometry — the boxes a
//! shape's `BakeShapeItem` produced once at client-mod load, reused for the
//! block-item's icon, dropped entity, and in-hand form. Keyed by block id
//! (stable for a session); populated by [`ClientModRuntime::bake_item_geometry`]
//! and read by [`render::item_cube`]'s custom branch. A miss (no client bake,
//! trapped, or empty) falls back to the block's plain cube there.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::block::Aabb;

static CACHE: OnceLock<Mutex<HashMap<u8, Arc<[Aabb]>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<u8, Arc<[Aabb]>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record a custom block's baked item boxes (cell-local, `0.0..1.0`).
pub(crate) fn set_item_bake(block_id: u8, boxes: Vec<Aabb>) {
    cache().lock().expect("item bake cache").insert(block_id, boxes.into());
}

/// The baked item boxes for a custom block, or `None` if it never baked one.
pub(crate) fn item_bake(block_id: u8) -> Option<Arc<[Aabb]>> {
    cache().lock().expect("item bake cache").get(&block_id).cloned()
}

/// Drop all cached item geometry. Block ids are session-local, so this cache
/// must be flushed when a world scene tears down or a stale entry could hand the
/// next session's block the wrong shape's item form.
pub(crate) fn clear() {
    cache().lock().expect("item bake cache").clear();
}
