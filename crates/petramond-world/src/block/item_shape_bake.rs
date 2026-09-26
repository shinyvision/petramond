//! Client cache of custom shapes' baked ITEM geometry — the boxes a
//! shape's `BakeShapeItem` produced once at client-mod load, reused for the
//! block-item's icon, dropped entity, and in-hand form. Keyed by block id and
//! kept per content registry (ids mean nothing across registries); populated by `ClientModRuntime::bake_item_geometry`
//! and read by `render::item_cube`'s custom branch. A miss (no client bake,
//! trapped, or empty) falls back to the block's plain cube there.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::block::Aabb;

type Cache = Mutex<HashMap<u16, Arc<[Aabb]>>>;

static CACHE: crate::content::Slot<Cache> =
    crate::content::Slot::new("item shape bakes", &[], empty_cache);

fn empty_cache(_: &crate::content::ContentRegistry) -> Result<Cache, String> {
    Ok(Mutex::default())
}

fn cache() -> &'static Cache {
    CACHE.current()
}

/// Record a custom block's baked item boxes (cell-local, `0.0..1.0`).
pub fn set_item_bake(block_id: u16, boxes: Vec<Aabb>) {
    cache()
        .lock()
        .expect("item bake cache")
        .insert(block_id, boxes.into());
}

/// The baked item boxes for a custom block, or `None` if it never baked one.
pub fn item_bake(block_id: u16) -> Option<Arc<[Aabb]>> {
    cache()
        .lock()
        .expect("item bake cache")
        .get(&block_id)
        .cloned()
}

/// Drop all cached item geometry. Block ids are session-local, so this cache
/// must be flushed when a world scene tears down or a stale entry could hand the
/// next session's block the wrong shape's item form.
pub fn clear() {
    cache().lock().expect("item bake cache").clear();
}
