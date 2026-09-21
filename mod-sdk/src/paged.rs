//! Batched host calls over more items than one call carries.

use crate::SIM_BATCH_MAX;

/// Run a batched host call over any number of `items`: split at the host's
/// per-call cap ([`SIM_BATCH_MAX`]) and join the replies in order.
///
/// `call` must answer one reply per item, as every `*_many` / batched SDK
/// call does:
///
/// ```ignore
/// let blocks = paged(cells, get_blocks);
/// ```
pub fn paged<T, R>(items: Vec<T>, mut call: impl FnMut(Vec<T>) -> Vec<R>) -> Vec<R> {
    if items.len() <= SIM_BATCH_MAX {
        return call(items);
    }
    let mut out = Vec::with_capacity(items.len());
    let mut rest = items;
    while !rest.is_empty() {
        let tail = rest.split_off(rest.len().min(SIM_BATCH_MAX));
        out.extend(call(rest));
        rest = tail;
    }
    out
}
