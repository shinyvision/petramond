use std::io;
use std::path::{Component, Path};

use petramond_persist::atomic_file;
use petramond_persist::bytecodec::Reader;

use super::region::{self, MergePolicy};

const MAGIC: &[u8; 4] = b"PMJ2";
const NAME: &str = "journal.bin";
const REGION_DIR: &str = "region";

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Region {
        rx: i32,
        rz: i32,
        records: Vec<(u16, Vec<u8>)>,
    },
    File {
        path: String,
        bytes: Vec<u8>,
    },
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Crash {
    TornJournal,
    AfterCommit,
    MidApply(usize),
}

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
    let mut appended = std::collections::BTreeSet::new();
    for entry in entries {
        match entry {
            Entry::Region { rx, rz, records } => {
                let store_dir = dir.join(REGION_DIR);
                std::fs::create_dir_all(&store_dir)?;
                let path = region::region_path(&store_dir, *rx, *rz);
                merge_records(&path, records)?;
                appended.insert(path);
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
    appended.iter().try_for_each(|path| region::sync(path))
}

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
    let mut r = Reader::new(body);
    let len = |r: &mut Reader| usize::try_from(r.u64()?).ok();
    let count = len(&mut r)?;
    let mut entries = Vec::new();
    for _ in 0..count {
        match r.u8()? {
            0 => {
                let rx = r.u32()? as i32;
                let rz = r.u32()? as i32;
                let n = len(&mut r)?;
                let mut records = Vec::new();
                for _ in 0..n {
                    let index = r.u16()?;
                    let body_len = len(&mut r)?;
                    records.push((index, r.bytes(body_len)?.to_vec()));
                }
                entries.push(Entry::Region { rx, rz, records });
            }
            1 => {
                let path_len = len(&mut r)?;
                let path = String::from_utf8(r.bytes(path_len)?.to_vec()).ok()?;
                if !stays_inside(&path) {
                    return None;
                }
                let bytes_len = len(&mut r)?;
                entries.push(Entry::File {
                    path,
                    bytes: r.bytes(bytes_len)?.to_vec(),
                });
            }
            _ => return None,
        }
    }
    r.is_at_end().then_some(entries)
}

fn stays_inside(path: &str) -> bool {
    let mut components = Path::new(path).components().peekable();
    components.peek().is_some() && components.all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests;
