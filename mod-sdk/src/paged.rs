//! Batched host calls over more items than one call carries.

use crate::SIM_BATCH_MAX;

/// Run a batched host call over any number of `items`: split at the host's
/// per-call cap ([`SIM_BATCH_MAX`]) and join the replies in order. No items
/// is no crossing at all.
///
/// Every batched call a mod makes over a list it does not bound itself goes
/// through here, because an over-cap batch is not a slow call — it is a
/// `HostRet::Error`, which the SDK turns into a guest panic and the host into
/// a DISABLED MOD.
///
/// `call` must answer one reply per item, as every `*_many` / batched SDK
/// call does:
///
/// ```ignore
/// let blocks = paged(cells, get_blocks);
/// ```
pub fn paged<T, R>(items: Vec<T>, mut call: impl FnMut(Vec<T>) -> Vec<R>) -> Vec<R> {
    if items.is_empty() {
        return Vec::new();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_are_split_at_the_host_cap_and_keep_their_order() {
        let items: Vec<usize> = (0..SIM_BATCH_MAX * 2 + 7).collect();
        let mut pages = Vec::new();
        let out = paged(items.clone(), |page| {
            pages.push(page.len());
            page.iter().map(|i| i * 2).collect()
        });
        assert_eq!(pages, [SIM_BATCH_MAX, SIM_BATCH_MAX, 7]);
        assert_eq!(out.len(), items.len(), "every element is answered");
        assert!(out.iter().enumerate().all(|(i, &v)| v == i * 2), "order held");
    }

    #[test]
    fn under_the_cap_is_one_call_and_nothing_is_no_call() {
        let mut calls = 0;
        paged(vec![1, 2, 3], |page| {
            calls += 1;
            page
        });
        assert_eq!(calls, 1);
        paged(Vec::<u8>::new(), |page| {
            calls += 1;
            page
        });
        assert_eq!(calls, 1, "an empty batch crossed the ABI");
    }
}
