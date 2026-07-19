use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};

use crate::item::{ItemStack, ItemType};

/// Sequential little-endian reader. Every read is bounds-checked and returns
/// `None` past the end, so a truncated / corrupt file fails cleanly.
pub struct Reader<'a> {
    bytes: &'a [u8],
    off: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, off: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.off.checked_add(n)?;
        let s = self.bytes.get(self.off..end)?;
        self.off = end;
        Some(s)
    }
    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.arr()?))
    }
    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.arr()?))
    }
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.arr()?))
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        self.take(n)
    }
}

pub(crate) fn put_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

pub(crate) fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

// Io-stream counterparts of [`Reader`] / `put_*`, for container files (regions)
// that seek past record bodies instead of slurping the whole file into a slice.
// Same widths, same little-endian order — keep them paired with the above.

pub(crate) fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0u8; 2];
    r.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

pub(crate) fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

pub(crate) fn write_u16(w: &mut impl Write, v: u16) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

pub(crate) fn write_u32(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

/// Encode one inventory/container slot as `[item id, count]`, with `[0, 0]` for an
/// empty or absent slot. Shared by the `level` (inventory/cursor) and `furnace`
/// codecs so the 2-byte slot format lives in exactly one place.
pub fn put_item_slot(buf: &mut Vec<u8>, slot: Option<ItemStack>) {
    match slot {
        Some(s) if !s.is_empty() => {
            put_u8(buf, super::palette::active().item_to_disk(s.item.id()));
            put_u8(buf, s.count);
        }
        _ => {
            put_u8(buf, 0);
            put_u8(buf, 0);
        }
    }
}

/// Decode a slot written by [`put_item_slot`]: `None` on truncated input,
/// `Some(None)` for an empty slot, else the stack.
pub fn get_item_slot(r: &mut Reader) -> Option<Option<ItemStack>> {
    let id = r.u8()?;
    let count = r.u8()?;
    if id == 0 || count == 0 {
        Some(None)
    } else {
        let id = super::palette::active().item_from_disk(id);
        Some(Some(ItemStack::new(ItemType::from_id(id), count)))
    }
}

/// Append a `u16`-length-prefixed list of `(local index, record)` entries to
/// `buf`, in ascending index order so identical state encodes identically. Owns
/// only the list FRAME — the count (capped at `u16::MAX`, since a chunk never
/// holds anywhere near that many), the sort-by-index reproducibility invariant,
/// the `2 + n * rec_bytes` reserve, and the per-entry `u16` index — and defers
/// the record body to `body`. Shared by the furnace and chest codecs.
pub(crate) fn put_indexed<T>(
    buf: &mut Vec<u8>,
    map: &HashMap<u16, T>,
    rec_bytes: usize,
    mut body: impl FnMut(&mut Vec<u8>, &T),
) {
    let n = map.len().min(u16::MAX as usize);
    buf.reserve(2 + n * rec_bytes);
    put_u16(buf, n as u16);
    let mut entries: Vec<(&u16, &T)> = map.iter().take(n).collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (idx, rec) in entries {
        put_u16(buf, *idx);
        body(buf, rec);
    }
}

/// Read an indexed list written by [`put_indexed`]: the `u16` count, then each
/// `u16` index paired with a record decoded by `body`. `None` on truncated
/// input (propagated from either the index read or `body`).
pub(crate) fn get_indexed<T>(
    r: &mut Reader,
    mut body: impl FnMut(&mut Reader) -> Option<T>,
) -> Option<HashMap<u16, T>> {
    let n = r.u16()? as usize;
    let mut out = HashMap::with_capacity(n.min(256));
    for _ in 0..n {
        let idx = r.u16()?;
        out.insert(idx, body(r)?);
    }
    Some(out)
}

/// Append a mod KV map: `u16` entry count, then per entry a `u16`-length-
/// prefixed key + `u32`-length-prefixed value. BTreeMap iteration is sorted,
/// so identical maps encode identically (the determinism the byte-exact
/// preservation contract rests on). An entry with an oversized key (> u16 —
/// the HostCall boundary caps keys far below this) is skipped defensively.
/// Shared by the per-cell (section), per-mob, and world (`level.dat`) KV payloads.
pub(crate) fn put_kv_map(buf: &mut Vec<u8>, map: &BTreeMap<String, Vec<u8>>) {
    let entries: Vec<(&String, &Vec<u8>)> = map
        .iter()
        .filter(|(k, _)| k.len() <= u16::MAX as usize)
        .take(u16::MAX as usize)
        .collect();
    put_u16(buf, entries.len() as u16);
    for (k, v) in entries {
        put_u16(buf, k.len() as u16);
        buf.extend_from_slice(k.as_bytes());
        put_u32(buf, v.len() as u32);
        buf.extend_from_slice(v);
    }
}

/// Read a mod KV map written by [`put_kv_map`]; `None` on truncated or
/// non-UTF-8 input.
pub(crate) fn get_kv_map(r: &mut Reader) -> Option<BTreeMap<String, Vec<u8>>> {
    let n = r.u16()? as usize;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let klen = r.u16()? as usize;
        let key = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let vlen = r.u32()? as usize;
        out.insert(key, r.bytes(vlen)?.to_vec());
    }
    Some(out)
}

/// zlib-compress a payload.
pub fn deflate(payload: &[u8]) -> Vec<u8> {
    // Explored-terrain persistence writes thousands of small records while the
    // player is moving. Level 1 preserves the zlib format while avoiding a full
    // core of background compression for marginal size gains on palette-like data.
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = e.write_all(payload);
    e.finish().unwrap_or_default()
}

/// zlib-decompress; `None` on corrupt input.
pub fn inflate(blob: &[u8]) -> Option<Vec<u8>> {
    let mut d = flate2::read::ZlibDecoder::new(blob);
    let mut out = Vec::new();
    d.read_to_end(&mut out).ok()?;
    Some(out)
}
