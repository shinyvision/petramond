use std::{cell::RefCell, collections::VecDeque};

use super::*;

const MAX_ENTRIES: usize = 256;
const MAX_CELL_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    feature: String,
    origin: [i32; 3],
    seed: u32,
    salt: u64,
}

#[derive(Default)]
struct Cache {
    entries: BTreeMap<Key, Arc<PlacedFeature>>,
    order: VecDeque<Key>,
    cell_bytes: usize,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

pub(super) fn resolve(
    feature: &str,
    origin: [i32; 3],
    seed: u32,
    salt: u64,
) -> Result<Arc<PlacedFeature>, String> {
    let key = Key {
        feature: feature.into(),
        origin,
        seed,
        salt,
    };
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(placed) = cache.entries.get(&key) {
            return Ok(Arc::clone(placed));
        }
        let placed = Arc::new(PlacedFeature::resolve(feature, origin, seed, salt)?);
        let bytes = placed.cells.capacity() * std::mem::size_of::<(IVec3, Block)>();
        while !cache.order.is_empty()
            && (cache.entries.len() >= MAX_ENTRIES || cache.cell_bytes + bytes > MAX_CELL_BYTES)
        {
            let oldest = cache.order.pop_front().unwrap();
            if let Some(old) = cache.entries.remove(&oldest) {
                cache.cell_bytes -= old.cells.capacity() * std::mem::size_of::<(IVec3, Block)>();
            }
        }
        if bytes <= MAX_CELL_BYTES {
            cache.cell_bytes += bytes;
            cache.order.push_back(key.clone());
            cache.entries.insert(key, Arc::clone(&placed));
        }
        Ok(placed)
    })
}
