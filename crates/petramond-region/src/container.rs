//! The region container itself: reading v2 and v3 files, appending to a v3
//! file, and compacting (see the crate docs for the format and its rules).

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use rustc_hash::FxHashMap;

use petramond_util::atomic_file::{self, Durability};
use petramond_util::bytecodec::{read_u16, read_u32, Reader};

const MAGIC_V2: u32 = 0x3252_434C; // "LCR2" little-endian
const MAGIC_V3: u32 = 0x3352_434C; // "LCR3" little-endian
const VERSION_V2: u16 = 2;
const VERSION_V3: u16 = 3;
/// v2: magic + version + record count, then per record `lidx u16 + len u32`.
const V2_RECORD_HEADER_BYTES: u64 = 6;
const V2_HEADER_BYTES: u64 = 8;
/// v3: magic + version + a reserved `u16`, then the two index slots.
const V3_PREFIX_BYTES: u64 = 8;
const SLOT_BYTES: usize = 32;
/// Where a v3 file's record bodies (and appended indexes) begin.
const DATA_START: u64 = V3_PREFIX_BYTES + 2 * SLOT_BYTES as u64;
/// Per v3 index entry: `lidx u16 + offset u64 + len u32`.
const INDEX_ENTRY_BYTES: usize = 14;
/// Garbage below this never triggers a compaction: small regions are cheap
/// to append to and not worth rewriting.
const COMPACT_FLOOR: u64 = 256 << 10;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RecordLocation {
    offset: u64,
    len: u32,
}

/// One index slot of a v3 header: where the index it names lives, its
/// checksum, and the slot's sequence number (the higher valid one wins).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Slot {
    seq: u64,
    index_offset: u64,
    index_len: u32,
    index_hash: u64,
}

impl Slot {
    fn offset(which: usize) -> u64 {
        V3_PREFIX_BYTES + (which * SLOT_BYTES) as u64
    }

    fn to_bytes(self) -> [u8; SLOT_BYTES] {
        let mut out = [0u8; SLOT_BYTES];
        out[0..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..16].copy_from_slice(&self.index_offset.to_le_bytes());
        out[16..20].copy_from_slice(&self.index_len.to_le_bytes());
        out[20..28].copy_from_slice(&self.index_hash.to_le_bytes());
        let check = fnv1a(&out[..28]) as u32;
        out[28..32].copy_from_slice(&check.to_le_bytes());
        out
    }

    /// `None` for a slot never written (all zeros) or one whose bytes are
    /// torn.
    fn from_bytes(b: &[u8]) -> Option<Slot> {
        let mut r = Reader::new(b);
        let (seq, index_offset, index_len, index_hash, check) =
            (r.u64()?, r.u64()?, r.u32()?, r.u64()?, r.u32()?);
        if check != fnv1a(&b[..28]) as u32 || seq == 0 {
            return None;
        }
        Some(Slot {
            seq,
            index_offset,
            index_len,
            index_hash,
        })
    }
}

/// FNV-1a: a checksum that tells a written slot or index from torn or
/// stale bytes. Not a defence against deliberate tampering.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// How the file behind a reader is laid out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Layout {
    /// No file: an empty region.
    Absent,
    /// A v2 file; the next merge rewrites it as v3.
    V2,
    /// A v3 file whose index came from slot `active`.
    V3 { active: usize, seq: u64 },
}

/// An open region plus its compact record index. Opening reads ONLY the
/// header and the index — record bodies are never touched until a targeted
/// `read_record`. The save thread keeps several of these readers in an LRU;
/// a reader stays valid while the file is appended to, because nothing it
/// indexed is ever overwritten in place.
pub struct RegionReader {
    file: Option<File>,
    records: FxHashMap<u16, RecordLocation>,
    layout: Layout,
    file_len: u64,
}

impl RegionReader {
    fn empty() -> Self {
        Self {
            file: None,
            records: FxHashMap::default(),
            layout: Layout::Absent,
            file_len: 0,
        }
    }

    /// Missing files are valid empty regions. Every recorded body is bounds-checked
    /// while indexing so a later targeted read cannot seek outside the container.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(e) => return Err(e),
        };
        // A file too short for its own header or table is as corrupt as a bad
        // magic.
        Self::index(file).map_err(|e| match e.kind() {
            io::ErrorKind::UnexpectedEof => corrupt_region(),
            _ => e,
        })
    }

    fn index(mut file: File) -> io::Result<Self> {
        let file_len = file.metadata()?.len();
        let mut prefix = [0u8; 8];
        file.read_exact(&mut prefix)?;
        let magic = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
        let version = u16::from_le_bytes([prefix[4], prefix[5]]);
        let (records, layout) = match (magic, version) {
            (MAGIC_V3, VERSION_V3) => index_v3(&mut file, file_len)?,
            (MAGIC_V2, VERSION_V2) => {
                let count = u16::from_le_bytes([prefix[6], prefix[7]]) as usize;
                (index_v2(&mut file, file_len, count)?, Layout::V2)
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported region file",
                ))
            }
        };
        Ok(Self {
            file: Some(file),
            records,
            layout,
            file_len,
        })
    }

    pub fn indices(&self) -> impl Iterator<Item = u16> + '_ {
        self.records.keys().copied()
    }

    /// Read one compressed body. Other records remain in the kernel page cache and
    /// are not copied into userspace.
    pub fn read_record(&mut self, lidx: u16) -> io::Result<Option<Vec<u8>>> {
        let Some(loc) = self.records.get(&lidx).copied() else {
            return Ok(None);
        };
        let file = self.file.as_mut().ok_or_else(corrupt_region)?;
        file.seek(SeekFrom::Start(loc.offset))?;
        let mut out = vec![0u8; loc.len as usize];
        file.read_exact(&mut out)?;
        Ok(Some(out))
    }

    fn live_bytes(&self) -> u64 {
        self.records.values().map(|l| u64::from(l.len)).sum()
    }

    /// Whether merging `replacements` should compact instead of append:
    /// the garbage left after the append would outweigh the live records
    /// (and the floor).
    fn compacts_after(&self, replacements: &BTreeMap<u16, Vec<u8>>) -> bool {
        let replaced: u64 = replacements
            .keys()
            .filter_map(|k| self.records.get(k))
            .map(|l| u64::from(l.len))
            .sum();
        let added: u64 = replacements.values().map(|b| b.len() as u64).sum();
        let live_after = self.live_bytes() - replaced + added;
        let garbage_after = (self.file_len + added).saturating_sub(DATA_START + live_after);
        garbage_after > live_after.max(COMPACT_FLOOR)
    }
}

/// The v2 table: contiguous `(lidx, len)` headers, then the bodies packed in
/// the same order.
fn index_v2(
    file: &mut File,
    file_len: u64,
    count: usize,
) -> io::Result<FxHashMap<u16, RecordLocation>> {
    let table_bytes = V2_RECORD_HEADER_BYTES
        .checked_mul(count as u64)
        .ok_or_else(corrupt_region)?;
    let body_base = V2_HEADER_BYTES
        .checked_add(table_bytes)
        .ok_or_else(corrupt_region)?;
    if body_base > file_len {
        return Err(corrupt_region());
    }
    let mut r = io::BufReader::with_capacity(64 << 10, file);
    let mut records = FxHashMap::with_capacity_and_hasher(count, rustc_hash::FxBuildHasher);
    let mut body_offset = body_base;
    for _ in 0..count {
        let lidx = read_u16(&mut r)?;
        let len = read_u32(&mut r)?;
        let end = body_offset
            .checked_add(u64::from(len))
            .ok_or_else(corrupt_region)?;
        let fresh = records
            .insert(
                lidx,
                RecordLocation {
                    offset: body_offset,
                    len,
                },
            )
            .is_none();
        if end > file_len || !fresh {
            return Err(corrupt_region());
        }
        body_offset = end;
    }
    if body_offset != file_len {
        return Err(corrupt_region());
    }
    Ok(records)
}

/// The v3 index named by the newest slot that checks out; an older slot
/// stands in when the newest one (or its index) is torn.
fn index_v3(
    file: &mut File,
    file_len: u64,
) -> io::Result<(FxHashMap<u16, RecordLocation>, Layout)> {
    if file_len < DATA_START {
        return Err(corrupt_region());
    }
    let mut slots = [0u8; 2 * SLOT_BYTES];
    file.read_exact(&mut slots)?;
    let mut candidates: Vec<(usize, Slot)> = (0..2)
        .filter_map(|i| Slot::from_bytes(&slots[i * SLOT_BYTES..(i + 1) * SLOT_BYTES]).map(|s| (i, s)))
        .collect();
    candidates.sort_by_key(|&(_, slot)| std::cmp::Reverse(slot.seq));
    for (active, slot) in candidates {
        if let Some(records) = read_index(file, file_len, slot)? {
            return Ok((
                records,
                Layout::V3 {
                    active,
                    seq: slot.seq,
                },
            ));
        }
    }
    Err(corrupt_region())
}

/// The index `slot` names, or `None` when it is out of bounds, fails its
/// checksum, or does not parse.
fn read_index(
    file: &mut File,
    file_len: u64,
    slot: Slot,
) -> io::Result<Option<FxHashMap<u16, RecordLocation>>> {
    let in_bounds = slot
        .index_offset
        .checked_add(u64::from(slot.index_len))
        .is_some_and(|end| end <= file_len);
    if slot.index_offset < DATA_START || !in_bounds {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(slot.index_offset))?;
    let mut bytes = vec![0u8; slot.index_len as usize];
    file.read_exact(&mut bytes)?;
    if fnv1a(&bytes) != slot.index_hash {
        return Ok(None);
    }
    Ok(parse_index(&bytes, slot.index_offset))
}

/// Every body an index names lies between the header and the index itself
/// (bodies are always written before the index that points at them).
fn parse_index(bytes: &[u8], index_offset: u64) -> Option<FxHashMap<u16, RecordLocation>> {
    let mut r = Reader::new(bytes);
    let count = r.u32()? as usize;
    if bytes.len() != count.checked_mul(INDEX_ENTRY_BYTES)?.checked_add(4)? {
        return None;
    }
    let mut records = FxHashMap::with_capacity_and_hasher(count, rustc_hash::FxBuildHasher);
    for _ in 0..count {
        let (lidx, offset, len) = (r.u16()?, r.u64()?, r.u32()?);
        let end = offset.checked_add(u64::from(len))?;
        if offset < DATA_START || end > index_offset {
            return None;
        }
        if records.insert(lidx, RecordLocation { offset, len }).is_some() {
            return None;
        }
    }
    Some(records)
}

fn encode_index(records: &BTreeMap<u16, RecordLocation>) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + records.len() * INDEX_ENTRY_BYTES);
    out.extend((records.len() as u32).to_le_bytes());
    for (lidx, loc) in records {
        out.extend(lidx.to_le_bytes());
        out.extend(loc.offset.to_le_bytes());
        out.extend(loc.len.to_le_bytes());
    }
    out
}

fn corrupt_region() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "corrupt region file")
}

fn too_large(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what)
}

/// The present local indices only (for building the load manifest cheaply).
pub fn read_region_indices(path: &Path) -> io::Result<Vec<u16>> {
    let mut indices: Vec<_> = RegionReader::open(path)?.indices().collect();
    indices.sort_unstable();
    Ok(indices)
}

/// What a region file's records are worth, which decides how carefully a
/// merge treats the old file and the new one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MergePolicy {
    /// The records exist nowhere else. An old file that cannot be read fails
    /// the merge (starting fresh would drop every record not being
    /// replaced), and a compaction flushes the new file to disk before it
    /// replaces the old one. Appends are left for the caller's [`sync`].
    Durable,
    /// The records can be rebuilt. An unreadable old file starts fresh and
    /// nothing is flushed.
    Rebuildable,
}

/// Merge replacement records into a region: appended to a v3 file (bodies,
/// then a new index, then the inactive header slot), or — for a new file, a
/// v2 file, an unreadable rebuildable one, or one whose garbage has grown
/// past its live data — compacted into a fresh v3 file that atomically
/// replaces the old one.
pub fn merge_region(
    path: &Path,
    replacements: impl IntoIterator<Item = (u16, Vec<u8>)>,
    policy: MergePolicy,
) -> io::Result<()> {
    let replacements: BTreeMap<u16, Vec<u8>> = replacements.into_iter().collect();
    // Nothing to write: leave the existing file alone (or the absent path as
    // absent). Callers occasionally flush empty batches through this path.
    if replacements.is_empty() {
        return Ok(());
    }
    if replacements.values().any(|body| u32::try_from(body.len()).is_err()) {
        return Err(too_large("region record too large"));
    }
    let old = match (RegionReader::open(path), policy) {
        (Ok(old), _) => old,
        (Err(e), MergePolicy::Durable) => return Err(e),
        (Err(_), MergePolicy::Rebuildable) => RegionReader::empty(),
    };
    match old.layout {
        Layout::V3 { active, seq } if !old.compacts_after(&replacements) => {
            append(path, &old, active, seq, &replacements)
        }
        _ => compact(path, old, replacements, policy),
    }
}

/// Append `replacements` and a new index covering every live record, then
/// point the inactive slot at it. Nothing is flushed (see [`sync`]).
fn append(
    path: &Path,
    old: &RegionReader,
    active: usize,
    seq: u64,
    replacements: &BTreeMap<u16, Vec<u8>>,
) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    // Past any bytes a crashed append left behind: they are garbage no
    // slot names, and compaction drops them.
    let mut offset = file.seek(SeekFrom::End(0))?;
    let mut records: BTreeMap<u16, RecordLocation> =
        old.records.iter().map(|(&lidx, &loc)| (lidx, loc)).collect();
    let index = {
        let mut out = BufWriter::with_capacity(256 << 10, &mut file);
        for (&lidx, body) in replacements {
            out.write_all(body)?;
            records.insert(
                lidx,
                RecordLocation {
                    offset,
                    len: body.len() as u32,
                },
            );
            offset += body.len() as u64;
        }
        let index = encode_index(&records);
        out.write_all(&index)?;
        out.flush()?;
        index
    };
    let slot = Slot {
        seq: seq + 1,
        index_offset: offset,
        index_len: u32::try_from(index.len()).map_err(|_| too_large("region index too large"))?,
        index_hash: fnv1a(&index),
    };
    file.seek(SeekFrom::Start(Slot::offset(1 - active)))?;
    file.write_all(&slot.to_bytes())
}

/// Write every live record (replacements from RAM, the rest copied from the
/// old file) into a fresh, garbage-free v3 file that atomically replaces
/// the old one.
fn compact(
    path: &Path,
    mut old: RegionReader,
    mut replacements: BTreeMap<u16, Vec<u8>>,
    policy: MergePolicy,
) -> io::Result<()> {
    let mut keys: Vec<u16> = old.indices().chain(replacements.keys().copied()).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut records = BTreeMap::new();
    let mut offset = DATA_START;
    for &lidx in &keys {
        let len = match replacements.get(&lidx) {
            Some(body) => body.len() as u32,
            None => old.records.get(&lidx).ok_or_else(corrupt_region)?.len,
        };
        records.insert(lidx, RecordLocation { offset, len });
        offset += u64::from(len);
    }
    let index = encode_index(&records);
    let slot = Slot {
        seq: 1,
        index_offset: offset,
        index_len: u32::try_from(index.len()).map_err(|_| too_large("region index too large"))?,
        index_hash: fnv1a(&index),
    };
    let durability = match policy {
        MergePolicy::Durable => Durability::Synced,
        MergePolicy::Rebuildable => Durability::Unsynced,
    };
    atomic_file::replace_with(path, durability, |file| {
        let mut out = BufWriter::with_capacity(256 << 10, file);
        out.write_all(&MAGIC_V3.to_le_bytes())?;
        out.write_all(&VERSION_V3.to_le_bytes())?;
        out.write_all(&0u16.to_le_bytes())?;
        out.write_all(&slot.to_bytes())?;
        out.write_all(&[0u8; SLOT_BYTES])?;
        let mut copy_buf = vec![0u8; 64 << 10];
        for lidx in keys {
            if let Some(record) = replacements.remove(&lidx) {
                out.write_all(&record)?;
                continue;
            }
            let loc = old.records.get(&lidx).copied().ok_or_else(corrupt_region)?;
            let file = old.file.as_mut().ok_or_else(corrupt_region)?;
            file.seek(SeekFrom::Start(loc.offset))?;
            let mut remaining = u64::from(loc.len);
            while remaining > 0 {
                let chunk = remaining.min(copy_buf.len() as u64) as usize;
                file.read_exact(&mut copy_buf[..chunk])?;
                out.write_all(&copy_buf[..chunk])?;
                remaining -= chunk as u64;
            }
        }
        out.write_all(&index)?;
        out.flush()
    })
}

/// Flush a region's appended bytes to disk. [`merge_region`] never flushes
/// an append itself, so a caller writing several regions for one durable
/// batch syncs each touched file once, after all of them are written.
pub fn sync(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

#[cfg(test)]
mod tests;
