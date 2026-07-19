//! Region files: each `r.<rx>.<rz>.dat` packs the modified sections of a 32×32
//! block of columns (a full vertical stack per column). Only sections the player
//! has modified are stored; the rest regenerate from the seed.
//!
//! A region is rewritten whole on every flush (new records merged over the
//! existing ones), which keeps the format trivial — contiguous header table of
//! present sections + packed compressed bodies, no slot map / free list /
//! compaction.
//!
//! Format v2 (2026-07-19): headers are contiguous so a manifest scan reads only
//! `8 + 6×count` bytes instead of streaming every compressed body. v1 files
//! (interleaved header/body) are rejected — wipe/regenerate. Same container is
//! shared by `region/`, `explored/`, and `colgen/`.
//!
//! Each section's slot is a `u16` local index packing its position within the
//! region: `lx` (5 bits) | `lz` (5 bits) | `cy − SECTION_MIN_CY` (5 bits). The
//! vertical range is `[SECTION_MIN_CY, SECTION_MAX_CY]` (20 sections), so the
//! biased `cy` fits in 5 bits and the whole index in a `u16`.

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::codec::{read_u16, read_u32, write_u16, write_u32};
use crate::chunk::{SectionPos, SECTION_MIN_CY};

/// Columns per region edge (32 → 1024 columns per region, each a vertical stack).
pub const REGION_SHIFT: i32 = 5;
pub const REGION_SIZE: i32 = 1 << REGION_SHIFT;
/// Bit width of each packed field in the `u16` local index.
const FIELD_SHIFT: u16 = 5;

const MAGIC: u32 = 0x3252_434C; // "LCR2" little-endian (cubic section records)
/// Contiguous header table + packed bodies. v1 (interleaved) is rejected.
const VERSION: u16 = 2;
const HEADER_BYTES: u64 = 8; // magic + version + count
const RECORD_HEADER_BYTES: u64 = 6; // lidx u16 + len u32

pub fn region_of(pos: SectionPos) -> (i32, i32) {
    (pos.cx >> REGION_SHIFT, pos.cz >> REGION_SHIFT)
}

pub fn local_index(pos: SectionPos) -> u16 {
    let lx = (pos.cx & (REGION_SIZE - 1)) as u16;
    let lz = (pos.cz & (REGION_SIZE - 1)) as u16;
    let ly = (pos.cy - SECTION_MIN_CY) as u16;
    (ly << (2 * FIELD_SHIFT)) | (lz << FIELD_SHIFT) | lx
}

pub fn section_pos(rx: i32, rz: i32, lidx: u16) -> SectionPos {
    let mask = (REGION_SIZE - 1) as u16;
    let lx = (lidx & mask) as i32;
    let lz = ((lidx >> FIELD_SHIFT) & mask) as i32;
    let cy = (lidx >> (2 * FIELD_SHIFT)) as i32 + SECTION_MIN_CY;
    SectionPos::new(rx * REGION_SIZE + lx, cy, rz * REGION_SIZE + lz)
}

pub fn region_path(region_dir: &Path, rx: i32, rz: i32) -> PathBuf {
    region_dir.join(format!("r.{rx}.{rz}.dat"))
}

/// Parse `r.<rx>.<rz>.dat` back into region coords (handles negatives).
pub fn parse_region_name(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("r.")?.strip_suffix(".dat")?;
    let (a, b) = rest.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

#[derive(Copy, Clone)]
struct RecordLocation {
    offset: u64,
    len: u32,
}

/// An open region plus its compact record index. Opening reads ONLY the
/// contiguous header table — compressed bodies are never touched until a
/// targeted `read_record`. The save thread keeps several of these readers in
/// an LRU.
pub(super) struct RegionReader {
    file: Option<File>,
    records: FxHashMap<u16, RecordLocation>,
}

impl RegionReader {
    /// Missing files are valid empty regions. Every recorded body is bounds-checked
    /// while indexing so a later targeted read cannot seek outside the container.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    file: None,
                    records: FxHashMap::default(),
                });
            }
            Err(e) => return Err(e),
        };
        let file_len = file.metadata()?.len();
        let mut r = io::BufReader::with_capacity(64 << 10, file);
        let magic = read_u32(&mut r)?;
        let version = read_u16(&mut r)?;
        if magic != MAGIC || version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported region file",
            ));
        }
        let count = read_u16(&mut r)? as usize;
        let table_bytes = RECORD_HEADER_BYTES
            .checked_mul(count as u64)
            .ok_or_else(corrupt_region)?;
        let body_base = HEADER_BYTES
            .checked_add(table_bytes)
            .ok_or_else(corrupt_region)?;
        if body_base > file_len {
            return Err(corrupt_region());
        }

        let mut records = FxHashMap::with_capacity_and_hasher(count, rustc_hash::FxBuildHasher);
        let mut body_offset = body_base;
        for _ in 0..count {
            let lidx = read_u16(&mut r)?;
            let len = read_u32(&mut r)?;
            let end = body_offset
                .checked_add(len as u64)
                .ok_or_else(corrupt_region)?;
            if end > file_len
                || records
                    .insert(lidx, RecordLocation {
                        offset: body_offset,
                        len,
                    })
                    .is_some()
            {
                return Err(corrupt_region());
            }
            body_offset = end;
        }
        if body_offset != file_len {
            return Err(corrupt_region());
        }
        Ok(Self {
            file: Some(r.into_inner()),
            records,
        })
    }

    pub(super) fn indices(&self) -> impl Iterator<Item = u16> + '_ {
        self.records.keys().copied()
    }

    /// Read one compressed body. Other records remain in the kernel page cache and
    /// are not copied into userspace.
    pub(super) fn read_record(&mut self, lidx: u16) -> io::Result<Option<Vec<u8>>> {
        let Some(loc) = self.records.get(&lidx).copied() else {
            return Ok(None);
        };
        let file = self.file.as_mut().ok_or_else(corrupt_region)?;
        file.seek(SeekFrom::Start(loc.offset))?;
        let mut out = vec![0u8; loc.len as usize];
        file.read_exact(&mut out)?;
        Ok(Some(out))
    }
}

fn corrupt_region() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "corrupt region file")
}

/// The present local indices only (for building the load manifest cheaply).
pub fn read_region_indices(path: &Path) -> io::Result<Vec<u16>> {
    let mut indices: Vec<_> = RegionReader::open(path)?.indices().collect();
    indices.sort_unstable();
    Ok(indices)
}

/// Merge replacement records into a region while copying every unchanged
/// compressed body directly from the old file to the new one. This keeps a flush
/// proportional to bytes written without allocating the old region as a map of
/// blobs. A corrupt old region starts fresh, matching the previous save policy.
pub(super) fn merge_region(
    path: &Path,
    replacements: impl IntoIterator<Item = (u16, Vec<u8>)>,
) -> io::Result<()> {
    let mut replacements: BTreeMap<u16, Vec<u8>> = replacements.into_iter().collect();
    // Nothing to rewrite: leave the existing file alone (or the absent path as
    // absent). Callers occasionally flush empty batches through this path.
    if replacements.is_empty() {
        return Ok(());
    }
    let mut old = RegionReader::open(path).unwrap_or_else(|_| RegionReader {
        file: None,
        records: FxHashMap::default(),
    });
    let mut keys: Vec<u16> = old.indices().collect();
    keys.extend(replacements.keys().copied());
    keys.sort_unstable();
    keys.dedup();
    if keys.len() > u16::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many region records",
        ));
    }

    let tmp = path.with_extension("tmp");
    let mut out = BufWriter::with_capacity(256 << 10, File::create(&tmp)?);
    write_u32(&mut out, MAGIC)?;
    write_u16(&mut out, VERSION)?;
    write_u16(&mut out, keys.len() as u16)?;
    // Pass 1: contiguous header table (lengths known without reading bodies).
    for &lidx in &keys {
        let len = replacements
            .get(&lidx)
            .map(|b| b.len() as u32)
            .or_else(|| old.records.get(&lidx).map(|loc| loc.len))
            .ok_or_else(corrupt_region)?;
        write_u16(&mut out, lidx)?;
        write_u32(&mut out, len)?;
    }
    // Pass 2: packed bodies in the same order — replacements from RAM,
    // unchanged records seek+copied from the old file.
    let mut copy_buf = [0u8; 64 << 10];
    for lidx in keys {
        if let Some(record) = replacements.remove(&lidx) {
            out.write_all(&record)?;
            continue;
        }
        let loc = old.records.get(&lidx).copied().ok_or_else(corrupt_region)?;
        let file = old.file.as_mut().ok_or_else(corrupt_region)?;
        file.seek(SeekFrom::Start(loc.offset))?;
        let mut remaining = loc.len as u64;
        while remaining > 0 {
            let chunk = remaining.min(copy_buf.len() as u64) as usize;
            file.read_exact(&mut copy_buf[..chunk])?;
            out.write_all(&copy_buf[..chunk])?;
            remaining -= chunk as u64;
        }
    }
    out.flush()?;
    drop(out);
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_math_roundtrips() {
        // Sweep XZ (incl. negatives + region edges) across the full cy range so the
        // packed (lx | lz | cy) local index inverts back to the same section.
        for &(cx, cz) in &[(0, 0), (-1, -1), (31, 31), (-32, 32), (100, -77)] {
            for cy in crate::chunk::SECTION_MIN_CY..=crate::chunk::SECTION_MAX_CY {
                let pos = SectionPos::new(cx, cy, cz);
                let (rx, rz) = region_of(pos);
                let lidx = local_index(pos);
                assert_eq!(section_pos(rx, rz, lidx), pos, "({cx},{cy},{cz})");
            }
        }
    }

    #[test]
    fn name_parse_roundtrips() {
        let dir = Path::new("/tmp/region");
        for &(rx, rz) in &[(0, 0), (-3, 5), (12, -1)] {
            let p = region_path(dir, rx, rz);
            assert_eq!(parse_region_name(&p), Some((rx, rz)));
        }
    }

    #[test]
    fn empty_merge_is_a_noop() {
        let path = std::env::temp_dir().join(format!(
            "petramond-region-empty-{}-{}.dat",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&path);
        merge_region(&path, []).expect("empty merge");
        assert!(
            !path.exists(),
            "an empty replacement set must not create a region file"
        );
    }

    #[test]
    fn indexed_merge_preserves_untouched_records() {
        let path = std::env::temp_dir().join(format!(
            "petramond-region-{}-{}.dat",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&path);

        merge_region(&path, [(9, vec![1, 2, 3]), (2, vec![4, 5])]).expect("initial write");
        assert_eq!(
            read_region_indices(&path).expect("index"),
            vec![2, 9],
            "container order is stable"
        );

        merge_region(&path, [(9, vec![8, 7, 6, 5])]).expect("merge");
        let mut reader = RegionReader::open(&path).expect("open merged region");
        assert_eq!(reader.read_record(2).unwrap().unwrap(), vec![4, 5]);
        assert_eq!(reader.read_record(9).unwrap().unwrap(), vec![8, 7, 6, 5]);
        assert_eq!(reader.read_record(100).unwrap(), None);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn v2_header_table_is_contiguous_before_bodies() {
        let path = std::env::temp_dir().join(format!(
            "petramond-region-v2-{}-{}.dat",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&path);
        // Three records with distinct bodies so layout is unambiguous.
        merge_region(
            &path,
            [
                (1, vec![0xAA; 10]),
                (2, vec![0xBB; 20]),
                (3, vec![0xCC; 30]),
            ],
        )
        .expect("write");
        let bytes = fs::read(&path).expect("read");
        // magic(4)+ver(2)+count(2) + 3×(lidx+len) = 8 + 18 = 26 header bytes,
        // then bodies 10+20+30 = 60 → total 86.
        assert_eq!(bytes.len(), 86);
        assert_eq!(&bytes[0..4], &MAGIC.to_le_bytes());
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), VERSION);
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 3);
        // First body byte sits immediately after the header table.
        assert_eq!(bytes[26], 0xAA);
        assert_eq!(bytes[36], 0xBB);
        assert_eq!(bytes[56], 0xCC);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn v1_interleaved_files_are_rejected() {
        let path = std::env::temp_dir().join(format!(
            "petramond-region-v1-{}-{}.dat",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&path);
        let mut out = File::create(&path).unwrap();
        write_u32(&mut out, MAGIC).unwrap();
        write_u16(&mut out, 1).unwrap(); // old interleaved version
        write_u16(&mut out, 1).unwrap();
        write_u16(&mut out, 7).unwrap();
        write_u32(&mut out, 3).unwrap();
        out.write_all(&[1, 2, 3]).unwrap();
        drop(out);
        match RegionReader::open(&path) {
            Err(err) => assert_eq!(err.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("v1 interleaved region must be rejected"),
        }
        let _ = fs::remove_file(path);
    }
}
