//! Client-mod KV storage with ordered background writes and reads.
//!
//! Guests address exact namespaced keys in batches. Synchronous reads exist
//! for small startup/edit lookups; BULK reads go through ticket-based
//! asynchronous requests on the same ordered worker, so neither map
//! exploration nor a zoomed-out viewport ever performs filesystem operations
//! on the app frame. Reads enqueue behind already-queued writes, which makes
//! read-your-writes ordering structural.
//!
//! A write is ticketed: one worker commits every write in the order it was
//! queued, so a bucket's outcomes arrive in ticket order and "where does
//! ticket N stand" is a watermark plus the tickets the disk refused.
//!
//! Nothing here is capped: a key is as long as the OS lets its hex file name
//! be (a refusal comes back in its words), a value and a batch as large as
//! the mod passes, and the queue is never full. The one rule is the reply's:
//! an answer larger than the guest can address could never be delivered.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, LazyLock};

/// One key's write in a batch: `None` deletes it; the ticket it belongs to.
type KeyedWrite = (String, Option<Arc<[u8]>>, u64);

/// A queued write's revision and value; `None` is a delete.
type PendingValue = (u64, Option<Arc<[u8]>>);
type Values = Vec<Option<Vec<u8>>>;
type ReadResult = Result<Values, String>;

/// What the worker reports for one committed write batch.
struct WriteDone {
    ticket: u64,
    revisions: Vec<(String, u64)>,
    outcome: Result<(), String>,
}

pub(super) struct ClientStorage {
    dir: PathBuf,
    pending: BTreeMap<String, PendingValue>,
    next_revision: u64,
    done_tx: mpsc::Sender<WriteDone>,
    done_rx: mpsc::Receiver<WriteDone>,
    next_write_ticket: u64,
    /// Every write ticket up to this one has landed.
    written_through: u64,
    /// The tickets the disk refused, kept for the bucket's life so an
    /// answer never changes once given: runs of consecutive tickets refused
    /// for one reason, by first ticket → (last ticket, reason). A disk that
    /// refuses every write for an hour is one run.
    failed_writes: BTreeMap<u64, (u64, Arc<str>)>,
    next_ticket: u64,
    in_flight_reads: HashSet<u64>,
    ready_reads: HashMap<u64, ReadResult>,
    read_tx: mpsc::Sender<(u64, ReadResult)>,
    read_rx: mpsc::Receiver<(u64, ReadResult)>,
}

/// `Err` when `values` could never be delivered to a guest that addresses
/// `reply_max` bytes.
fn deliverable(values: &Values, reply_max: u64) -> Result<(), String> {
    let total: u64 = values
        .iter()
        .flatten()
        .map(|value| value.len() as u64)
        .sum();
    if total > reply_max {
        return Err(format!(
            "client storage read of {total} bytes exceeds the {reply_max} bytes this              instance can address; read fewer keys at once"
        ));
    }
    Ok(())
}

impl ClientStorage {
    /// The bucket's directory: its KV blobs, and its mod files under `files/`.
    pub(in crate::modding) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(super) fn new(dir: PathBuf) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        let (read_tx, read_rx) = mpsc::channel();
        Self {
            dir,
            pending: BTreeMap::new(),
            next_revision: 1,
            done_tx,
            done_rx,
            next_write_ticket: 1,
            written_through: 0,
            failed_writes: BTreeMap::new(),
            next_ticket: 1,
            in_flight_reads: HashSet::new(),
            ready_reads: HashMap::new(),
            read_tx,
            read_rx,
        }
    }

    /// Queue an asynchronous read on the storage worker. The worker processes
    /// the request after every already-queued write has committed (one FIFO
    /// channel), so a begun read always observes this session's earlier
    /// writes.
    pub(super) fn read_begin(&mut self, keys: Vec<String>) -> Result<u64, String> {
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        let message = StorageMessage::Read {
            dir: self.dir.clone(),
            keys,
            ticket,
            results: self.read_tx.clone(),
        };
        if storage_worker().send(message).is_err() {
            return Err("client storage worker stopped".into());
        }
        self.in_flight_reads.insert(ticket);
        Ok(ticket)
    }

    /// `Ok(Some(values))` consumes the ticket; `Ok(None)` = still in flight.
    pub(super) fn read_poll(
        &mut self,
        ticket: u64,
        reply_max: u64,
    ) -> Result<Option<Values>, String> {
        self.drain_read_completions();
        if let Some(result) = self.ready_reads.remove(&ticket) {
            let values = result?;
            deliverable(&values, reply_max)?;
            return Ok(Some(values));
        }
        if self.in_flight_reads.contains(&ticket) {
            return Ok(None);
        }
        Err(format!("unknown client storage read ticket {ticket}"))
    }

    fn drain_read_completions(&mut self) {
        while let Ok((ticket, result)) = self.read_rx.try_recv() {
            self.in_flight_reads.remove(&ticket);
            self.ready_reads.insert(ticket, result);
        }
    }

    pub(super) fn get_many(&mut self, keys: &[String], reply_max: u64) -> ReadResult {
        self.drain_completions();
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let value = match self.pending.get(key) {
                Some((_, value)) => value.as_deref().map(<[u8]>::to_vec),
                None => read_value(&self.dir.join(hex_key(key)))?,
            };
            out.push(value);
        }
        deliverable(&out, reply_max)?;
        Ok(out)
    }

    /// Queue a batch of writes (`None` = delete) and answer its ticket.
    /// `Err` = the worker is gone.
    pub(super) fn set_many(
        &mut self,
        entries: Vec<(String, Option<Vec<u8>>)>,
    ) -> Result<u64, String> {
        self.drain_completions();
        let ticket = self.next_write_ticket;
        if entries.is_empty() {
            // Nothing to write lands at once — but only once every earlier
            // ticket has, or the watermark would claim them too.
            if self.written_through + 1 == ticket {
                self.written_through = ticket;
            } else {
                self.queue_write(ticket, Vec::new())?;
            }
            self.next_write_ticket += 1;
            return Ok(ticket);
        }

        let mut writes = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let revision = self.next_revision;
            self.next_revision = self.next_revision.wrapping_add(1).max(1);
            let value: Option<Arc<[u8]>> = value.map(|v| Arc::from(v.into_boxed_slice()));
            self.pending.insert(key.clone(), (revision, value.clone()));
            writes.push((key, value, revision));
        }
        self.queue_write(ticket, writes)?;
        self.next_write_ticket += 1;
        Ok(ticket)
    }

    fn queue_write(&mut self, ticket: u64, entries: Vec<KeyedWrite>) -> Result<(), String> {
        let message = StorageMessage::Write {
            dir: self.dir.clone(),
            ticket,
            entries,
            done: self.done_tx.clone(),
        };
        storage_worker()
            .send(message)
            .map_err(|_| "the storage worker stopped".to_owned())
    }

    /// Where write `ticket` stands: `None` = still queued, then whether the
    /// disk took it. A ticket never issued is an error.
    pub(super) fn write_poll(&mut self, ticket: u64) -> Result<Option<Result<(), String>>, String> {
        if ticket == 0 || ticket >= self.next_write_ticket {
            return Err(format!("unknown client storage write ticket {ticket}"));
        }
        self.drain_completions();
        if ticket > self.written_through {
            return Ok(None);
        }
        let refused = self
            .failed_writes
            .range(..=ticket)
            .next_back()
            .filter(|(_, (last, _))| *last >= ticket);
        Ok(Some(match refused {
            Some((_, (_, why))) => Err(why.to_string()),
            None => Ok(()),
        }))
    }

    fn drain_completions(&mut self) {
        while let Ok(done) = self.done_rx.try_recv() {
            for (key, revision) in done.revisions {
                if self
                    .pending
                    .get(&key)
                    .is_some_and(|(pending_revision, _)| *pending_revision == revision)
                {
                    self.pending.remove(&key);
                }
            }
            self.written_through = self.written_through.max(done.ticket);
            if let Err(why) = done.outcome {
                self.refused(done.ticket, why);
            }
        }
    }

    fn refused(&mut self, ticket: u64, why: String) {
        if let Some((_, (last, reason))) = self.failed_writes.iter_mut().next_back() {
            if *last + 1 == ticket && **reason == *why {
                *last = ticket;
                return;
            }
        }
        self.failed_writes.insert(ticket, (ticket, why.into()));
    }

    /// Wait until the worker has done everything queued before this call.
    #[cfg(test)]
    fn settle(&self) {
        let (done, wait) = mpsc::channel();
        if storage_worker().send(StorageMessage::Flush(done)).is_ok() {
            let _ = wait.recv();
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
        ticket: u64,
        entries: Vec<KeyedWrite>,
        done: mpsc::Sender<WriteDone>,
    },
    Read {
        dir: PathBuf,
        keys: Vec<String>,
        ticket: u64,
        results: mpsc::Sender<(u64, ReadResult)>,
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
                ticket,
                entries,
                done,
            } => {
                let outcome = write_many(&dir, &entries);
                if let Err(error) = &outcome {
                    log::error!("{error}");
                }
                let revisions = entries
                    .into_iter()
                    .map(|(key, _, revision)| (key, revision))
                    .collect();
                let _ = done.send(WriteDone {
                    ticket,
                    revisions,
                    outcome,
                });
            }
            StorageMessage::Read {
                dir,
                keys,
                ticket,
                results,
            } => {
                let _ = results.send((ticket, read_many(&dir, &keys)));
            }
            StorageMessage::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn read_many(dir: &Path, keys: &[String]) -> ReadResult {
    keys.iter()
        .map(|key| read_value(&dir.join(hex_key(key))))
        .collect()
}

fn write_many(dir: &Path, entries: &[KeyedWrite]) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("create client storage {}: {error}", dir.display()))?;
    for (key, value, _) in entries {
        let path = dir.join(hex_key(key));
        let Some(value) = value else {
            match std::fs::remove_file(&path) {
                Ok(()) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!("delete client storage {}: {error}", path.display()))
                }
            }
        };
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

    const ANY: u64 = u64::MAX;

    #[test]
    fn queued_writes_are_immediately_readable_and_persist_before_drop() {
        let dir = petramond_util::test_dirs::TestScratchDir::new("client-storage");
        {
            let mut storage = ClientStorage::new(dir.to_path_buf());
            assert!(storage
                .set_many(vec![("map:tile:0:0".into(), Some(vec![1, 2, 3]))])
                .is_ok());
            assert_eq!(
                storage.get_many(&["map:tile:0:0".into()], ANY).unwrap(),
                vec![Some(vec![1, 2, 3])]
            );
        }
        let mut reopened = ClientStorage::new(dir.to_path_buf());
        assert_eq!(
            reopened.get_many(&["map:tile:0:0".into()], ANY).unwrap(),
            vec![Some(vec![1, 2, 3])]
        );
    }

    /// The async-read contracts: reads see writes queued before them (one
    /// FIFO worker), a delivered result consumes its ticket, unknown tickets
    /// error, and an answer the guest could never address is an error rather
    /// than a reply.
    #[test]
    fn async_reads_are_ordered_after_writes_and_ticketed() {
        let dir = petramond_util::test_dirs::TestScratchDir::new("client-storage-async");
        let mut storage = ClientStorage::new(dir.to_path_buf());

        assert!(storage
            .set_many(vec![
                ("map:a".into(), Some(vec![1])),
                ("map:b".into(), Some(vec![2, 2])),
            ])
            .is_ok());
        let ticket = storage
            .read_begin(vec!["map:a".into(), "map:b".into(), "map:missing".into()])
            .unwrap();
        storage.settle();
        assert_eq!(
            storage.read_poll(ticket, ANY).unwrap(),
            Some(vec![Some(vec![1]), Some(vec![2, 2]), None]),
            "an async read begun after a write observes that write"
        );
        assert!(
            storage.read_poll(ticket, ANY).is_err(),
            "a delivered result consumes the ticket"
        );
        assert!(
            storage.read_poll(999, ANY).is_err(),
            "unknown tickets error"
        );

        let ticket = storage
            .read_begin(vec!["map:a".into(), "map:b".into()])
            .unwrap();
        storage.settle();
        assert!(storage.read_poll(ticket, 2).is_err());
        assert!(storage
            .get_many(&["map:a".into(), "map:b".into()], 2)
            .is_err());
        assert!(storage
            .get_many(&["map:a".into(), "map:b".into()], 3)
            .is_ok());
    }

    fn outcome(storage: &mut ClientStorage, ticket: u64) -> Result<(), String> {
        storage.settle();
        storage
            .write_poll(ticket)
            .unwrap()
            .expect("settled writes have landed")
    }

    /// `None` deletes (reads then answer absent, the pending copy included),
    /// empty bytes are a value, and a write's ticket reports what the DISK
    /// said — a refused directory is an `Err`, never a silent `Ok`, however
    /// long ago it was refused and however often it is asked.
    #[test]
    fn deletes_and_write_outcomes_are_reported_truthfully() {
        let root = petramond_util::test_dirs::TestScratchDir::new("client-storage-outcomes");
        let mut storage = ClientStorage::new(root.join("bucket"));
        let key = |k: &str| k.to_string();
        let ticket = storage
            .set_many(vec![
                (key("m:a"), Some(vec![1])),
                (key("m:e"), Some(Vec::new())),
            ])
            .unwrap();
        assert_eq!(outcome(&mut storage, ticket), Ok(()));
        let delete = storage.set_many(vec![(key("m:a"), None)]).unwrap();
        assert_eq!(
            storage.get_many(&[key("m:a"), key("m:e")], ANY).unwrap(),
            vec![None, Some(Vec::new())],
            "a queued delete reads as absent before it lands"
        );
        assert_eq!(outcome(&mut storage, delete), Ok(()));
        drop(storage);
        let mut reopened = ClientStorage::new(root.join("bucket"));
        assert_eq!(
            reopened.get_many(&[key("m:a"), key("m:e")], ANY).unwrap(),
            vec![None, Some(Vec::new())]
        );
        assert!(reopened.write_poll(99).is_err(), "a ticket never issued");

        std::fs::write(root.join("file"), b"x").unwrap();
        let mut refused = ClientStorage::new(root.join("file").join("bucket"));
        let tickets: Vec<u64> = (0..3)
            .map(|_| refused.set_many(vec![(key("m:a"), Some(vec![1]))]).unwrap())
            .collect();
        let empty = refused.set_many(Vec::new()).unwrap();
        let later = refused.set_many(vec![(key("m:b"), Some(vec![1]))]).unwrap();
        for _ in 0..2 {
            for ticket in &tickets {
                assert!(outcome(&mut refused, *ticket).is_err());
            }
            assert_eq!(outcome(&mut refused, empty), Ok(()));
            assert!(outcome(&mut refused, later).is_err());
        }
    }
}
