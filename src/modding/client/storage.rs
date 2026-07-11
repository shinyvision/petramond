//! Bounded client-mod storage with ordered background writes.
//!
//! Guests address exact namespaced keys in batches. Reads are bounded and
//! synchronous; writes enter one process-wide ordered worker so map exploration
//! never performs hundreds of filesystem operations on the app frame.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, LazyLock};

pub(super) const KEY_MAX: usize = 100;
pub(super) const VALUE_MAX: usize = 1 << 20;
pub(super) const BATCH_MAX: usize = 16 << 20;
pub(super) const READ_MAX: usize = 16 << 20;
pub(super) const GET_KEYS_MAX: usize = 4096;
const PENDING_MAX: usize = 32 << 20;

type PendingValue = (u64, Arc<[u8]>);

pub(super) struct ClientStorage {
    dir: PathBuf,
    pending: BTreeMap<String, PendingValue>,
    next_revision: u64,
    pending_bytes: Arc<AtomicUsize>,
    done_tx: mpsc::Sender<Vec<(String, u64)>>,
    done_rx: mpsc::Receiver<Vec<(String, u64)>>,
}

impl ClientStorage {
    pub(super) fn new(dir: PathBuf) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self {
            dir,
            pending: BTreeMap::new(),
            next_revision: 1,
            pending_bytes: Arc::new(AtomicUsize::new(0)),
            done_tx,
            done_rx,
        }
    }

    pub(super) fn get_many(&mut self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, String> {
        if keys.len() > GET_KEYS_MAX {
            return Err(format!(
                "client storage read has {} keys; cap is {GET_KEYS_MAX}",
                keys.len()
            ));
        }
        self.drain_completions();
        let mut total = 0usize;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if key.len() > KEY_MAX {
                return Err(format!(
                    "client storage key '{key}' exceeds {KEY_MAX} bytes"
                ));
            }
            let value = match self.pending.get(key) {
                Some((_, value)) => Some(value.to_vec()),
                None => read_value(&self.dir.join(hex_key(key)))?,
            };
            if let Some(value) = &value {
                total = total.saturating_add(key.len()).saturating_add(value.len());
                if total > READ_MAX {
                    return Err(format!(
                        "client storage read exceeds the {READ_MAX}-byte response cap"
                    ));
                }
            }
            out.push(value);
        }
        Ok(out)
    }

    pub(super) fn set_many(&mut self, entries: Vec<(String, Vec<u8>)>) -> Result<(), String> {
        self.drain_completions();
        let total = entries
            .iter()
            .map(|(key, value)| key.len().saturating_add(value.len()))
            .sum::<usize>();
        if total > BATCH_MAX {
            return Err(format!(
                "client storage batch is {total} bytes; cap is {BATCH_MAX}"
            ));
        }
        for (key, value) in &entries {
            if key.len() > KEY_MAX || value.len() > VALUE_MAX {
                return Err(format!(
                    "client storage entry '{key}' exceeds key/value limits"
                ));
            }
        }
        if self
            .pending_bytes
            .load(Ordering::Acquire)
            .saturating_add(total)
            > PENDING_MAX
        {
            return Err(format!(
                "client storage has too many queued writes; pending cap is {PENDING_MAX} bytes"
            ));
        }

        let mut writes = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let revision = self.next_revision;
            self.next_revision = self.next_revision.wrapping_add(1).max(1);
            let value: Arc<[u8]> = Arc::from(value.into_boxed_slice());
            self.pending.insert(key.clone(), (revision, value.clone()));
            writes.push((key, value, revision));
        }
        if writes.is_empty() {
            return Ok(());
        }

        self.pending_bytes.fetch_add(total, Ordering::AcqRel);
        let message = StorageMessage::Write {
            dir: self.dir.clone(),
            entries: writes,
            done: self.done_tx.clone(),
            pending_bytes: self.pending_bytes.clone(),
            bytes: total,
        };
        if storage_worker().send(message).is_err() {
            self.pending_bytes.fetch_sub(total, Ordering::AcqRel);
            return Err("client storage worker stopped".into());
        }
        Ok(())
    }

    fn drain_completions(&mut self) {
        while let Ok(done) = self.done_rx.try_recv() {
            for (key, revision) in done {
                if self
                    .pending
                    .get(&key)
                    .is_some_and(|(pending_revision, _)| *pending_revision == revision)
                {
                    self.pending.remove(&key);
                }
            }
        }
    }
}

impl Drop for ClientStorage {
    fn drop(&mut self) {
        let (done, wait) = mpsc::channel();
        if storage_worker().send(StorageMessage::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }
}

enum StorageMessage {
    Write {
        dir: PathBuf,
        entries: Vec<(String, Arc<[u8]>, u64)>,
        done: mpsc::Sender<Vec<(String, u64)>>,
        pending_bytes: Arc<AtomicUsize>,
        bytes: usize,
    },
    Flush(mpsc::Sender<()>),
}

fn storage_worker() -> &'static mpsc::Sender<StorageMessage> {
    static WORKER: LazyLock<mpsc::Sender<StorageMessage>> = LazyLock::new(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("client-mod-storage".into())
            .spawn(move || storage_worker_loop(rx))
            .expect("spawn client mod storage worker");
        tx
    });
    &WORKER
}

fn storage_worker_loop(rx: mpsc::Receiver<StorageMessage>) {
    while let Ok(message) = rx.recv() {
        match message {
            StorageMessage::Write {
                dir,
                entries,
                done,
                pending_bytes,
                bytes,
            } => {
                if let Err(error) = write_many(&dir, &entries) {
                    log::error!("{error}");
                }
                let completed = entries
                    .into_iter()
                    .map(|(key, _, revision)| (key, revision))
                    .collect();
                let _ = done.send(completed);
                pending_bytes.fetch_sub(bytes, Ordering::AcqRel);
            }
            StorageMessage::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn write_many(dir: &Path, entries: &[(String, Arc<[u8]>, u64)]) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("create client storage {}: {error}", dir.display()))?;
    for (key, value, _) in entries {
        let path = dir.join(hex_key(key));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, value)
            .map_err(|error| format!("write client storage {}: {error}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|error| format!("commit client storage {}: {error}", path.display()))?;
    }
    Ok(())
}

fn read_value(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read client storage {}: {error}", path.display())),
    }
}

fn hex_key(key: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(key.len() * 2);
    for byte in key.bytes() {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_writes_are_immediately_readable_and_persist_before_drop() {
        let dir =
            std::env::temp_dir().join(format!("petramond-client-storage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut storage = ClientStorage::new(dir.clone());
            storage
                .set_many(vec![("map:tile:0:0".into(), vec![1, 2, 3])])
                .unwrap();
            assert_eq!(
                storage.get_many(&["map:tile:0:0".into()]).unwrap(),
                vec![Some(vec![1, 2, 3])]
            );
        }
        let mut reopened = ClientStorage::new(dir.clone());
        assert_eq!(
            reopened.get_many(&["map:tile:0:0".into()]).unwrap(),
            vec![Some(vec![1, 2, 3])]
        );
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
    }
}
