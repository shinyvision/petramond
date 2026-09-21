//! Durable save batches: every authoritative write of one save lands on disk
//! together or not at all.
//!
//! A batch is first written whole to `journal.bin` (checksummed, flushed to
//! disk, then renamed into place), only then applied to the region, level and
//! player files, and only after every apply succeeded is the journal retired.
//! Opening a world finishes a journal left behind: a crash after the commit
//! replays the whole batch (region records are replacements, files are whole,
//! so replaying is idempotent), and a torn journal is discarded, leaving the
//! previous save intact. A write that fails keeps the journal, and nothing is
//! committed over it: the next write (or the next open) finishes it first.

use std::io;
use std::path::{Component, Path};

use petramond_util::atomic_file;

use super::region::{self, MergePolicy};

const MAGIC: &[u8; 4] = b"PMJ2";
const NAME: &str = "journal.bin";
/// The authoritative section store, relative to the world directory.
const REGION_DIR: &str = "region";

/// One write in a batch.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    /// Section records replacing theirs in the authoritative region file
    /// `(rx, rz)`.
    Region {
        rx: i32,
        rz: i32,
        records: Vec<(u16, Vec<u8>)>,
    },
    /// A whole file at a path relative to the world directory.
    File { path: String, bytes: Vec<u8> },
}

/// Stop points a test can make the batch writer halt at, as a crash would.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Crash {
    /// The journal file is only partly written.
    TornJournal,
    /// The journal is committed; nothing is applied.
    AfterCommit,
    /// Only the first `n` entries are applied.
    MidApply(usize),
}

/// Write `entries` durably: commit, apply, retire. An error after the commit
/// leaves the journal in place, and the batch is finished by the next
/// `write` or [`recover`] — nothing is committed over it.
pub(super) fn write(dir: &Path, entries: &[Entry]) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    recover(dir)?;
    commit(dir, entries)?;
    apply(dir, entries)?;
    retire(dir)
}

#[cfg(test)]
pub(super) fn write_crashing(dir: &Path, entries: &[Entry], crash: Crash) -> io::Result<()> {
    if let Crash::TornJournal = crash {
        let bytes = encode(entries);
        return std::fs::write(dir.join(NAME), &bytes[..bytes.len() / 2]);
    }
    commit(dir, entries)?;
    match crash {
        Crash::MidApply(n) => apply(dir, &entries[..n.min(entries.len())]),
        _ => Ok(()),
    }
}

/// Finish a batch that was committed but never retired. Returns whether one
/// was replayed; a torn or unreadable journal is removed.
pub(super) fn recover(dir: &Path) -> io::Result<bool> {
    let path = dir.join(NAME);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    match decode(&bytes) {
        Some(entries) => {
            apply(dir, &entries)?;
            retire(dir)?;
            Ok(true)
        }
        None => {
            log::warn!("discarding an incomplete save journal in {}", dir.display());
            std::fs::remove_file(&path)?;
            Ok(false)
        }
    }
}

fn commit(dir: &Path, entries: &[Entry]) -> io::Result<()> {
    atomic_file::replace(&dir.join(NAME), &encode(entries))
}

fn apply(dir: &Path, entries: &[Entry]) -> io::Result<()> {
    for entry in entries {
        match entry {
            Entry::Region { rx, rz, records } => {
                let store_dir = dir.join(REGION_DIR);
                std::fs::create_dir_all(&store_dir)?;
                merge_records(&region::region_path(&store_dir, *rx, *rz), records)?;
            }
            Entry::File { path, bytes } => {
                let path = dir.join(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                atomic_file::replace(&path, bytes)?;
            }
        }
    }
    Ok(())
}

/// Merge into a region whose other records must survive. A region file whose
/// CONTENT is bad is set aside under another name and the merge starts a
/// new one: no reader can get a record out of it either, so nothing
/// loadable is lost, the bytes are kept, and one bad file cannot stop the
/// world from ever saving again. Any other failure to read it is an error —
/// the records may be fine.
fn merge_records(path: &Path, records: &[(u16, Vec<u8>)]) -> io::Result<()> {
    match region::merge_region(path, records.iter().cloned(), MergePolicy::Durable) {
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            let aside = path.with_extension("dat.unreadable");
            log::error!(
                "region file {} is unreadable ({e}); kept as {}",
                path.display(),
                aside.display()
            );
            std::fs::rename(path, &aside)?;
            region::merge_region(path, records.iter().cloned(), MergePolicy::Durable)
        }
        result => result,
    }
}

fn retire(dir: &Path) -> io::Result<()> {
    std::fs::remove_file(dir.join(NAME))?;
    atomic_file::sync_dir(dir)
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    out.extend((len as u64).to_le_bytes());
}

fn encode(entries: &[Entry]) -> Vec<u8> {
    let mut body = Vec::new();
    put_len(&mut body, entries.len());
    for entry in entries {
        match entry {
            Entry::Region { rx, rz, records } => {
                body.push(0);
                body.extend(rx.to_le_bytes());
                body.extend(rz.to_le_bytes());
                put_len(&mut body, records.len());
                for (index, bytes) in records {
                    body.extend(index.to_le_bytes());
                    put_len(&mut body, bytes.len());
                    body.extend(bytes);
                }
            }
            Entry::File { path, bytes } => {
                body.push(1);
                put_len(&mut body, path.len());
                body.extend(path.as_bytes());
                put_len(&mut body, bytes.len());
                body.extend(bytes);
            }
        }
    }
    let mut out = Vec::with_capacity(body.len() + 40);
    out.extend(MAGIC);
    out.extend(blake3::hash(&body).as_bytes());
    out.extend(body);
    out
}

fn decode(bytes: &[u8]) -> Option<Vec<Entry>> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (hash, body) = rest.split_at_checked(32)?;
    if blake3::hash(body).as_bytes() != hash {
        return None;
    }
    let mut r = Reader(body);
    let count = r.len()?;
    let mut entries = Vec::new();
    for _ in 0..count {
        match r.take(1)?[0] {
            0 => {
                let rx = r.i32()?;
                let rz = r.i32()?;
                let n = r.len()?;
                let mut records = Vec::new();
                for _ in 0..n {
                    let index = u16::from_le_bytes(r.take(2)?.try_into().ok()?);
                    let len = r.len()?;
                    records.push((index, r.take(len)?.to_vec()));
                }
                entries.push(Entry::Region { rx, rz, records });
            }
            1 => {
                let len = r.len()?;
                let path = String::from_utf8(r.take(len)?.to_vec()).ok()?;
                if !stays_inside(&path) {
                    return None;
                }
                let len = r.len()?;
                entries.push(Entry::File {
                    path,
                    bytes: r.take(len)?.to_vec(),
                });
            }
            _ => return None,
        }
    }
    r.0.is_empty().then_some(entries)
}

/// A journal only ever names files inside the world directory.
fn stays_inside(path: &str) -> bool {
    let mut components = Path::new(path).components().peekable();
    components.peek().is_some() && components.all(|c| matches!(c, Component::Normal(_)))
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(n)?;
        self.0 = tail;
        Some(head)
    }

    fn len(&mut self) -> Option<usize> {
        usize::try_from(u64::from_le_bytes(self.take(8)?.try_into().ok()?)).ok()
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests;
