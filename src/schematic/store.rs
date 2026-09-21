//! A world's own schematic assets: complete `.llschematic` archives addressed
//! by the BLAKE3 digest of their bytes and published once under the world
//! save. A project refers to its design by digest, so deleting a personal
//! library file never breaks it, and a copy of the same design from anyone
//! is the same asset. Assets live as long as the world: a reference can sit
//! in an unloaded chest or an offline player's inventory, so nothing short of
//! a complete census could prove one unused.
//!
//! Decoding and file I/O run on worker threads; the owning thread polls.

use super::{archive, Schematic};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

#[cfg(test)]
mod tests;

pub use crate::net::blob::{digest, Digest};

/// How many decoded designs stay resident; the rest reload from disk.
const DECODED_RESIDENT: usize = 8;

pub fn hex(digest: &Digest) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A decoded asset: the design and its archive's small facts.
pub struct Asset {
    pub digest: Digest,
    pub schematic: Schematic,
    pub bytes_len: u64,
}

/// Where an asset stands for a reader.
pub enum Lookup {
    /// The world holds no asset under this digest.
    Missing,
    /// Decoding on a worker: ask again on a later tick.
    Loading,
    Ready(Arc<Asset>),
    /// The stored archive could not be read.
    Failed(String),
}

/// A finished background job the owner collects with [`Store::poll`].
pub enum Finished {
    /// An archive was validated and published under its digest.
    Published(Result<Digest, String>),
}

/// A publication's outcome, and the archive bytes when the world keeps its
/// assets in memory.
type Published = (Result<Digest, String>, Option<Arc<[u8]>>);

pub struct Store {
    /// `<world>/schematics`, or `None` for a world with no save (assets
    /// then live in memory for the session).
    dir: Option<PathBuf>,
    memory: HashMap<Digest, Arc<[u8]>>,
    decoded: HashMap<Digest, Arc<Asset>>,
    decoded_order: VecDeque<Digest>,
    loading: HashMap<Digest, mpsc::Receiver<Result<Asset, String>>>,
    failed: HashMap<Digest, String>,
    publishing: Vec<mpsc::Receiver<Published>>,
    /// Digests confirmed on disk, so readers polling every tick do not stat.
    known: std::cell::RefCell<std::collections::HashSet<Digest>>,
}

impl Store {
    pub fn new(world_dir: Option<&Path>) -> Self {
        Self {
            dir: world_dir.map(|d| d.join("schematics")),
            memory: HashMap::new(),
            decoded: HashMap::new(),
            decoded_order: VecDeque::new(),
            loading: HashMap::new(),
            failed: HashMap::new(),
            publishing: Vec::new(),
            known: Default::default(),
        }
    }

    fn path(&self, digest: &Digest) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{}.{}", hex(digest), archive::EXTENSION)))
    }

    /// Whether the world holds `digest`.
    pub fn contains(&self, digest: &Digest) -> bool {
        if self.memory.contains_key(digest)
            || self.decoded.contains_key(digest)
            || self.known.borrow().contains(digest)
        {
            return true;
        }
        let present = self.path(digest).is_some_and(|p| p.exists());
        if present {
            self.known.borrow_mut().insert(*digest);
        }
        present
    }

    /// The decoded asset, starting a background decode on first ask.
    pub fn lookup(&mut self, digest: &Digest) -> Lookup {
        if let Some(asset) = self.decoded.get(digest) {
            return Lookup::Ready(asset.clone());
        }
        if let Some(error) = self.failed.get(digest) {
            return Lookup::Failed(error.clone());
        }
        if let Some(rx) = self.loading.get(digest) {
            match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => return Lookup::Loading,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.loading.remove(digest);
                    self.failed
                        .insert(*digest, "Schematic decoding failed".into());
                    return Lookup::Failed("Schematic decoding failed".into());
                }
                Ok(Ok(asset)) => {
                    self.loading.remove(digest);
                    return Lookup::Ready(self.keep_decoded(asset));
                }
                Ok(Err(error)) => {
                    self.loading.remove(digest);
                    self.failed.insert(*digest, error.clone());
                    return Lookup::Failed(error);
                }
            }
        }
        if !self.contains(digest) {
            return Lookup::Missing;
        }
        let source = self.memory.get(digest).cloned();
        let path = self.path(digest);
        let digest = *digest;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let bytes: Arc<[u8]> = match source {
                    Some(bytes) => bytes,
                    None => std::fs::read(path.ok_or("No schematic store")?)
                        .map_err(|e| e.to_string())?
                        .into(),
                };
                decode_asset(digest, &bytes)
            })();
            let _ = tx.send(result);
        });
        self.loading.insert(digest, rx);
        Lookup::Loading
    }

    fn keep_decoded(&mut self, asset: Asset) -> Arc<Asset> {
        let digest = asset.digest;
        let asset = Arc::new(asset);
        self.decoded.insert(digest, asset.clone());
        self.decoded_order.push_back(digest);
        while self.decoded_order.len() > DECODED_RESIDENT {
            if let Some(old) = self.decoded_order.pop_front() {
                if old != digest {
                    self.decoded.remove(&old);
                }
            }
        }
        asset
    }

    /// A job reading the archive bytes of `digest`, to run off the owning
    /// thread.
    pub fn reader(&self, digest: Digest) -> impl FnOnce() -> Result<Arc<[u8]>, String> + Send {
        let memory = self.memory.get(&digest).cloned();
        let path = self.path(&digest);
        move || match memory {
            Some(bytes) => Ok(bytes),
            None => std::fs::read(path.ok_or("The world holds no such schematic")?)
                .map(Into::into)
                .map_err(|e| e.to_string()),
        }
    }

    /// The archive bytes of `digest`, for sending to a client.
    pub fn read_bytes(&self, digest: &Digest) -> Result<Arc<[u8]>, String> {
        if let Some(bytes) = self.memory.get(digest) {
            return Ok(bytes.clone());
        }
        let path = self
            .path(digest)
            .ok_or("The world holds no such schematic")?;
        std::fs::read(path)
            .map(Into::into)
            .map_err(|e| e.to_string())
    }

    /// Validate received archive bytes that claim to be `expected` and
    /// publish them, on a worker: the digest must match, the archive must
    /// decode completely, and the file is durably written before it is
    /// visible. The outcome arrives through [`poll`](Self::poll).
    pub fn publish(&mut self, expected: Digest, bytes: Vec<u8>) {
        let dir = self.dir.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let bytes: Arc<[u8]> = bytes.into();
            let result = (|| {
                if digest(&bytes) != expected {
                    return Err("The schematic arrived damaged".to_string());
                }
                archive::decode(&bytes)?;
                if let Some(dir) = dir {
                    publish_file(&dir, &expected, &bytes)?;
                }
                Ok(expected)
            })();
            let keep = result.is_ok().then_some(bytes);
            let _ = tx.send((result, keep));
        });
        self.publishing.push(rx);
    }

    /// Collect finished background publications.
    pub fn poll(&mut self) -> Vec<Finished> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.publishing.len() {
            match self.publishing[i].try_recv() {
                Err(mpsc::TryRecvError::Empty) => i += 1,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.publishing.swap_remove(i);
                    out.push(Finished::Published(Err("Schematic import failed".into())));
                }
                Ok((result, bytes)) => {
                    self.publishing.swap_remove(i);
                    if let (Ok(digest), Some(bytes)) = (&result, bytes) {
                        self.failed.remove(digest);
                        if self.dir.is_none() {
                            self.memory.insert(*digest, bytes);
                        }
                    }
                    out.push(Finished::Published(result));
                }
            }
        }
        out
    }
}

fn decode_asset(digest: Digest, bytes: &[u8]) -> Result<Asset, String> {
    Ok(Asset {
        digest,
        schematic: archive::decode(bytes)?,
        bytes_len: bytes.len() as u64,
    })
}

/// Publish `bytes` under their digest name, complete or not at all; an
/// existing file with the same digest already holds these exact bytes.
fn publish_file(dir: &Path, digest: &Digest, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let target = dir.join(format!("{}.{}", hex(digest), archive::EXTENSION));
    if target.exists() {
        return Ok(());
    }
    match petramond_util::atomic_file::publish_new(&target, |file| file.write_all(bytes)) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e.to_string()),
        _ => Ok(()),
    }
}
