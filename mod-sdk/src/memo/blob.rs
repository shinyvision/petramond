//! Paged derived values, published after all of their pages are available.

use crate::{ByteReader, ByteWriter, MemoClaim, MEMO_MAX_VALUE_BYTES};

const PAGE_BYTES: usize = MEMO_MAX_VALUE_BYTES;
const MAX_PAGES: usize = 64;

trait Access {
    fn claim(&mut self, key: &[u8]) -> MemoClaim;
    fn get_many(&mut self, keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>>;
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> bool;
}
struct Host;
impl Access for Host {
    fn claim(&mut self, key: &[u8]) -> MemoClaim {
        super::memo_claim(key)
    }
    fn get_many(&mut self, keys: Vec<Vec<u8>>) -> Vec<Option<Vec<u8>>> {
        super::memo_get_many(keys)
    }
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> bool {
        super::memo_put(key, value)
    }
}

/// Claim a derived value larger than a single memo entry. Reserve six bytes
/// of key headroom. Missing or evicted pages are re-derived under a page lease.
pub fn memo_blob_claim(key: &[u8]) -> MemoClaim {
    claim(&mut Host, key)
}

/// Publish up to 64 memo pages. The value remains discardable, like [`super::memo_put`].
pub fn memo_blob_put(key: &[u8], value: Vec<u8>) -> bool {
    put(&mut Host, key, value)
}

fn page_key(key: &[u8], page: usize) -> Vec<u8> {
    let mut k = ByteWriter::with_capacity(key.len() + 6);
    k.raw(key);
    k.raw(&[0, 1]);
    k.u32(page as u32);
    k.finish()
}

fn claim(a: &mut impl Access, key: &[u8]) -> MemoClaim {
    let bytes = match a.claim(key) {
        MemoClaim::Value(v) => v,
        other => return other,
    };
    if bytes.first() == Some(&0) {
        return MemoClaim::Value(bytes[1..].to_vec());
    }
    let mut r = ByteReader::new(&bytes);
    if r.take(1) != Some(&[1][..]) {
        return MemoClaim::Lease;
    }
    let Some(len) = r.u32().map(|n| n as usize) else {
        return MemoClaim::Lease;
    };
    let pages = len.div_ceil(PAGE_BYTES);
    if pages == 0 || pages > MAX_PAGES {
        return MemoClaim::Lease;
    }
    let keys: Vec<_> = (0..pages).map(|i| page_key(key, i)).collect();
    let mut parts = a.get_many(keys.clone());
    if parts.len() != pages {
        return MemoClaim::Pending;
    }
    for (i, part) in parts.iter_mut().enumerate() {
        if part.is_none() {
            match a.claim(&keys[i]) {
                MemoClaim::Value(v) => *part = Some(v),
                other => return other,
            }
        }
    }
    let mut value = Vec::with_capacity(len);
    for (i, part) in parts.into_iter().enumerate() {
        let part = part.expect("all pages available");
        if part.len() != (len - i * PAGE_BYTES).min(PAGE_BYTES) {
            return MemoClaim::Lease;
        }
        value.extend(part);
    }
    MemoClaim::Value(value)
}

fn put(a: &mut impl Access, key: &[u8], value: Vec<u8>) -> bool {
    if value.len() < PAGE_BYTES {
        let mut inline = Vec::with_capacity(value.len() + 1);
        inline.push(0);
        inline.extend(value);
        return a.put(key, inline);
    }
    if value.len() > MAX_PAGES * PAGE_BYTES {
        return false;
    }
    for (i, page) in value.chunks(PAGE_BYTES).enumerate() {
        if !a.put(&page_key(key, i), page.to_vec()) {
            return false;
        }
    }
    let mut manifest = ByteWriter::new();
    manifest.raw(&[1]);
    manifest.u32(value.len() as u32);
    a.put(key, manifest.finish())
}

#[cfg(test)]
mod tests;
