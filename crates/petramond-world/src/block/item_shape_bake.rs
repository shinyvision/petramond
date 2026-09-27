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

pub fn set_item_bake(block_id: u16, boxes: Vec<Aabb>) {
    cache()
        .lock()
        .expect("item bake cache")
        .insert(block_id, boxes.into());
}

pub fn item_bake(block_id: u16) -> Option<Arc<[Aabb]>> {
    cache()
        .lock()
        .expect("item bake cache")
        .get(&block_id)
        .cloned()
}

pub fn clear() {
    cache().lock().expect("item bake cache").clear();
}
