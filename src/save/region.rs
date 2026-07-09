//! Region files: each `r.<rx>.<rz>.dat` packs the modified sections of a 32×32
//! block of columns (a full vertical stack per column). Only sections the player
//! has modified are stored; the rest regenerate from the seed.
//!
//! A region is rewritten whole on every flush (new records merged over the
//! existing ones), which keeps the format trivial — header table of present
//! sections + their compressed records, no slot map / free list / compaction.
//!
//! Each section's slot is a `u16` local index packing its position within the
//! region: `lx` (5 bits) | `lz` (5 bits) | `cy − SECTION_MIN_CY` (5 bits). The
//! vertical range is `[SECTION_MIN_CY, SECTION_MAX_CY]` (20 sections), so the
//! biased `cy` fits in 5 bits and the whole index in a `u16`.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::chunk::{SectionPos, SECTION_MIN_CY};

/// Columns per region edge (32 → 1024 columns per region, each a vertical stack).
pub const REGION_SHIFT: i32 = 5;
pub const REGION_SIZE: i32 = 1 << REGION_SHIFT;
/// Bit width of each packed field in the `u16` local index.
const FIELD_SHIFT: u16 = 5;

const MAGIC: u32 = 0x3252_434C; // "LCR2" little-endian (cubic section records)
const VERSION: u16 = 1;

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

/// An open region plus its compact record index. Opening scans only the six-byte
/// record headers and seeks over compressed bodies; it never reads or copies an
/// unrelated record. The save thread keeps several of these readers in an LRU.
pub(super) struct RegionReader {
    file: Option<File>,
    records: HashMap<u16, RecordLocation>,
}

impl RegionReader {
    /// Missing files are valid empty regions. Every recorded body is bounds-checked
    /// while indexing so a later targeted read cannot seek outside the container.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    file: None,
                    records: HashMap::new(),
                });
            }
            Err(e) => return Err(e),
        };
        let file_len = file.metadata()?.len();
        let magic = read_u32(&mut file)?;
        let version = read_u16(&mut file)?;
        if magic != MAGIC || version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported region file",
            ));
        }
        let count = read_u16(&mut file)? as usize;
        let mut records = HashMap::with_capacity(count);
        for _ in 0..count {
            let lidx = read_u16(&mut file)?;
            let len = read_u32(&mut file)?;
            let offset = file.stream_position()?;
            let end = offset.checked_add(len as u64).ok_or_else(corrupt_region)?;
            if end > file_len
                || records
                    .insert(lidx, RecordLocation { offset, len })
                    .is_some()
            {
                return Err(corrupt_region());
            }
            file.seek(SeekFrom::Start(end))?;
        }
        Ok(Self {
            file: Some(file),
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

fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0u8; 2];
    r.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
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
    let mut old = RegionReader::open(path).unwrap_or_else(|_| RegionReader {
        file: None,
        records: HashMap::new(),
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
    let mut out = BufWriter::new(File::create(&tmp)?);
    out.write_all(&MAGIC.to_le_bytes())?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&(keys.len() as u16).to_le_bytes())?;
    for lidx in keys {
        out.write_all(&lidx.to_le_bytes())?;
        if let Some(record) = replacements.remove(&lidx) {
            out.write_all(&(record.len() as u32).to_le_bytes())?;
            out.write_all(&record)?;
            continue;
        }
        let loc = old.records.get(&lidx).copied().ok_or_else(corrupt_region)?;
        out.write_all(&loc.len.to_le_bytes())?;
        let file = old.file.as_mut().ok_or_else(corrupt_region)?;
        file.seek(SeekFrom::Start(loc.offset))?;
        let copied = io::copy(&mut file.take(loc.len as u64), &mut out)?;
        if copied != loc.len as u64 {
            return Err(corrupt_region());
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
}
