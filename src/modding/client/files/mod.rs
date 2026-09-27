//! A client mod's FILES in its storage buckets, and the one store every
//! writer of them goes through: the mod's own appends, writes, syncs,
//! renames, deletes, reads and listings, and the records the engine writes
//! into a mod's files for it.
//!
//! Every mutation of one file lands in submission order ([`queue`]); the I/O
//! threads ([`io`]) make the blocking calls, so nothing here touches the disk
//! on the frame except [`stat`]'s one metadata read of a file the store is
//! not writing. Nothing is counted or refused for its size: a producer that
//! outpaces the disk grows memory, and [`stat`]'s `len - written` lets the
//! mod act on its own backlog.
//!
//! Every file has an INCARNATION: a rename moves it, a delete ends it, and a
//! positioned write announces the bytes it changes ([`subscribe`]), so a
//! cache keyed by `(incarnation, offset, len)` is never stale.

pub mod folders;
mod io;
mod paths;
mod queue;
mod reveal;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};

use mod_api::{ClientFileAnswer, ClientFileInfo, ClientStorageScope};

pub use io::Listing;
use io::{At, Job};
use queue::{Disk, Effect, FileKey, Sched};

pub use reveal::os_helper_turn;

/// A fresh id for any ticket or handle a client instance is given: file
/// tickets, events logs, captures, applies, taps and media files share one
/// counter, so no two families ever answer to one id.
pub fn issue_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// One file or directory in one mod's bucket, as the mod named it.
#[derive(Clone, Debug)]
pub struct FileRef {
    root: Arc<Path>,
    rel: Arc<str>,
    key: FileKey,
}

impl FileRef {
    /// `rel` must already pass [`mod_api::file_path_problem`] (or be `""`,
    /// the bucket's root directory).
    fn new(root: Arc<Path>, rel: &str) -> Self {
        Self {
            key: FileKey {
                root: paths::root_id(&root),
                folded: paths::fold(rel).into(),
            },
            root,
            rel: rel.into(),
        }
    }

    /// `rel` in a bucket whose files live under `dir` — a test's scratch
    /// directory, never the player's data.
    #[cfg(test)]
    pub fn in_dir(dir: &Path, rel: &str) -> Self {
        Self::new(Arc::from(dir), rel)
    }

    /// `rel` (which passes the path rule) under the bucket directory `root`:
    /// a test's own bucket.
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_bucket(root: &Path, rel: &str) -> Self {
        Self::new(Arc::from(root), rel)
    }

    /// The path the mod named, for messages.
    pub fn rel(&self) -> &str {
        &self.rel
    }

    pub fn path(&self) -> PathBuf {
        paths::join(&self.root, &self.rel)
    }

    /// A sibling in the same bucket.
    pub fn sibling(&self, rel: &str) -> Self {
        Self::new(self.root.clone(), rel)
    }

    fn os(&self, error: impl std::fmt::Display) -> String {
        format!("{}: {error}", self.rel)
    }

    /// This file's incarnation: the same number until it is deleted or
    /// renamed over, whatever it is renamed to.
    pub fn incarnation(&self) -> u64 {
        shared().with(|store| store.incarnation(&self.key))
    }
}

/// What changed under an incarnation, announced at the call that changed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// Bytes `[start, end)` changed (`end` = `u64::MAX`: through the end).
    Overwritten { incarnation: u64, range: [u64; 2] },
    /// These incarnations ended: `path` was deleted, or renamed over.
    Ended {
        incarnations: Vec<u64>,
        path: String,
    },
}

/// Who writes a file from outside its queue, while they do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterKind {
    /// Appends only (an events log): the mod's own appends may interleave
    /// between its records, but a positioned write could land inside one.
    Appends(&'static str),
    /// Owns every byte (a media file's encoder).
    Exclusive(&'static str),
}

struct Store {
    sched: Sched<Job, Handle>,
    incarnations: BTreeMap<FileKey, u64>,
    next_incarnation: u64,
    writers: BTreeMap<FileKey, Vec<(u64, WriterKind)>>,
    subscribers: Vec<mpsc::Sender<FileChange>>,
    /// Jobs an I/O thread took and has not yet finished running, completion
    /// included.
    active: usize,
}

impl Store {
    fn incarnation(&mut self, key: &FileKey) -> u64 {
        let next = &mut self.next_incarnation;
        *self.incarnations.entry(key.clone()).or_insert_with(|| {
            *next += 1;
            *next
        })
    }

    fn emit(&mut self, change: FileChange) {
        self.subscribers
            .retain(|subscriber| subscriber.send(change.clone()).is_ok());
    }

    /// The incarnations of `key` and everything under it, ended.
    fn end_subtree(&mut self, key: &FileKey, path: &str) {
        let keys = queue::under(&self.incarnations, key);
        let incarnations: Vec<u64> = std::iter::once(key)
            .chain(&keys)
            .filter_map(|key| self.incarnations.remove(key))
            .collect();
        if !incarnations.is_empty() {
            self.emit(FileChange::Ended {
                incarnations,
                path: path.to_owned(),
            });
        }
    }

    /// Why an external writer keeps `file` from this operation: `whole` =
    /// the operation replaces or removes it and everything under it;
    /// `positioned` = it writes inside existing bytes.
    fn in_use(&self, file: &FileRef, whole: bool, positioned: bool) -> Result<(), String> {
        let keys = if whole {
            let mut keys = queue::under(&self.writers, &file.key);
            keys.push(file.key.clone());
            keys
        } else {
            vec![file.key.clone()]
        };
        for key in keys {
            for (_, kind) in self.writers.get(&key).into_iter().flatten() {
                let (blocks, what) = match *kind {
                    WriterKind::Appends(what) => (whole || positioned, what),
                    WriterKind::Exclusive(what) => (true, what),
                };
                if blocks {
                    return Err(format!(
                        "{} is being written by {what}; end it first",
                        file.rel
                    ));
                }
            }
        }
        Ok(())
    }
}

struct Shared {
    store: Mutex<Store>,
    work: Condvar,
    /// Told when the store has nothing queued or running.
    idle: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Change the queues, and wake the I/O threads for whatever became ready.
    fn with<R>(&self, change: impl FnOnce(&mut Store) -> R) -> R {
        let mut store = self.lock();
        let before = store.sched.ready_len();
        let result = change(&mut store);
        for _ in before..store.sched.ready_len() {
            self.work.notify_one();
        }
        result
    }

    /// Operation `id` ran: record what it did to its files, and move on.
    /// The I/O thread calling this takes the next ready work itself, so one
    /// fewer thread is woken for it.
    fn landed(&self, id: queue::OpId, record: impl FnOnce(&mut Store)) {
        let mut store = self.lock();
        let before = store.sched.ready_len() + 1;
        record(&mut store);
        store.sched.finish(id);
        for _ in before..store.sched.ready_len() {
            self.work.notify_one();
        }
    }

    fn finish(&self, id: queue::OpId) {
        self.landed(id, |_| {});
    }
}

fn shared() -> &'static Shared {
    static SHARED: LazyLock<Shared> = LazyLock::new(|| {
        io::spawn_threads();
        Shared {
            store: Mutex::new(Store {
                sched: Sched::default(),
                incarnations: BTreeMap::new(),
                next_incarnation: 0,
                writers: BTreeMap::new(),
                subscribers: Vec::new(),
                active: 0,
            }),
            work: Condvar::new(),
            idle: Condvar::new(),
        }
    });
    &SHARED
}

type Hook<T> = Box<dyn FnOnce(Result<T, String>) + Send>;

/// An open file and the exact name it was opened by: an operation naming
/// the file in another case must not reuse it past the case rule.
type Handle = (Arc<str>, Arc<File>);

/// Append `bytes` at the end of `file`, creating it and its directories.
/// `done` answers where they landed. `Err` = refused (an external writer
/// owns the file).
pub fn append(
    file: &FileRef,
    bytes: Vec<u8>,
    done: impl FnOnce(Result<[u64; 2], String>) + Send + 'static,
) -> Result<(), String> {
    let len = bytes.len() as u64;
    shared().with(|store| {
        store.in_use(file, false, false)?;
        let job = Job::Put {
            file: file.clone(),
            at: At::End,
            bytes: Some(Ok(vec![Chunk::from(bytes)])),
            record: false,
            poisoned: None,
            done: Box::new(done),
        };
        store
            .sched
            .mutate(&file.key, Effect::Grow(Some(len)), job, false);
        Ok(())
    })
}

/// Put `bytes` at `offset` of `file` (a gap fills with zeros); `truncate`
/// then cuts it at `offset + bytes.len()`.
pub fn write(
    file: &FileRef,
    offset: u64,
    bytes: Vec<u8>,
    truncate: bool,
    done: impl FnOnce(Result<[u64; 2], String>) + Send + 'static,
) -> Result<(), String> {
    let len = bytes.len() as u64;
    shared().with(|store| {
        store.in_use(file, false, true)?;
        if let Some(&incarnation) = store.incarnations.get(&file.key) {
            let end = if truncate { u64::MAX } else { offset + len };
            store.emit(FileChange::Overwritten {
                incarnation,
                range: [offset, end],
            });
        }
        let job = Job::Put {
            file: file.clone(),
            at: At::Offset { offset, truncate },
            bytes: Some(Ok(vec![Chunk::from(bytes)])),
            record: false,
            poisoned: None,
            done: Box::new(done),
        };
        let effect = Effect::Put {
            offset,
            len,
            truncate,
        };
        store.sched.mutate(&file.key, effect, job, false);
        Ok(())
    })
}

/// Put `bytes` at `offset` of `file` in its turn: how the engine completes a
/// record it streamed there (its head, rewritten in place). Unlike
/// [`write`], an external writer never refuses it: the bytes are the
/// engine's own record, and no one else's record can lie under them.
pub fn patch(
    file: &FileRef,
    offset: u64,
    bytes: Vec<u8>,
    done: impl FnOnce(Result<[u64; 2], String>) + Send + 'static,
) {
    let len = bytes.len() as u64;
    shared().with(|store| {
        if let Some(&incarnation) = store.incarnations.get(&file.key) {
            store.emit(FileChange::Overwritten {
                incarnation,
                range: [offset, offset + len],
            });
        }
        let job = Job::Put {
            file: file.clone(),
            at: At::Offset {
                offset,
                truncate: false,
            },
            bytes: Some(Ok(vec![Chunk::from(bytes)])),
            record: false,
            poisoned: None,
            done: Box::new(done),
        };
        let effect = Effect::Put {
            offset,
            len,
            truncate: false,
        };
        store.sched.mutate(&file.key, effect, job, false);
    });
}

/// fsync `file` once everything queued to it before this has landed.
pub fn sync(file: &FileRef, done: impl FnOnce(Result<(), String>) + Send + 'static) {
    queue_sync(file, false, Box::new(done));
}

fn queue_sync(file: &FileRef, if_dirty: bool, done: Hook<()>) {
    shared().with(|store| {
        let job = Job::Sync {
            file: file.clone(),
            if_dirty,
            done,
        };
        store.sched.mutate(&file.key, Effect::Sync, job, false);
    });
}

/// Rename `from` onto `to` after everything queued to either: `from` is
/// synced first, an existing file at `to` is replaced atomically, and the
/// incarnation moves with the content.
pub fn rename(
    from: &FileRef,
    to: &FileRef,
    done: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    shared().with(|store| {
        store.in_use(from, true, true)?;
        store.in_use(to, true, true)?;
        store.end_subtree(&to.key, &to.rel);
        let moved: Vec<(FileKey, u64)> = std::iter::once(from.key.clone())
            .chain(queue::under(&store.incarnations, &from.key))
            .filter_map(|key| store.incarnations.remove(&key).map(|n| (key, n)))
            .collect();
        for (key, incarnation) in moved {
            let tail = &key.folded[from.key.folded.len()..];
            let key = FileKey {
                root: to.key.root,
                folded: format!("{}{tail}", to.key.folded).into(),
            };
            store.incarnations.insert(key, incarnation);
        }
        let job = Job::Rename {
            from: from.clone(),
            to: to.clone(),
            done: Box::new(done),
        };
        store.sched.relocate(&from.key, &to.key, job);
        Ok(())
    })
}

/// Delete a file, or a directory with everything under it, after what is
/// queued under it. Its incarnations end NOW, and reads still waiting there
/// fail by name.
pub fn delete(
    file: &FileRef,
    done: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    let failed = shared().with(|store| {
        store.in_use(file, true, true)?;
        store.end_subtree(&file.key, &file.rel);
        let job = Job::Delete {
            file: file.clone(),
            done: Box::new(done),
        };
        Ok::<_, String>(store.sched.delete(&file.key, job).1)
    })?;
    for job in failed {
        let path = match &job {
            Job::Read { file, .. } | Job::List { dir: file, .. } => file.rel.to_string(),
            _ => file.rel.to_string(),
        };
        job.fail(format!("{path} was deleted"));
    }
    Ok(())
}

/// Read `[offset, offset + len)` of `file`: the bytes that exist there, after
/// exactly the queued writes that change them.
pub fn read(
    file: &FileRef,
    offset: u64,
    len: u64,
    done: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
) {
    shared().with(|store| {
        let job = Job::Read {
            file: file.clone(),
            offset,
            len,
            done: Box::new(done),
        };
        store.sched.read(&file.key, offset, len, job);
    });
}

/// One page of `dir` (`rel` `""` = the bucket's root), sorted by name,
/// after `after`, while the encoded answer stays within `max_bytes` (always
/// one entry while any remain).
pub fn list(
    dir: &FileRef,
    after: Option<String>,
    max_bytes: u64,
    done: impl FnOnce(Result<Listing, String>) + Send + 'static,
) {
    shared().with(|store| {
        let job = Job::List {
            dir: dir.clone(),
            after,
            max_bytes,
            done: Box::new(done),
        };
        store.sched.list(&dir.key, job);
    });
}

/// `file` as it stands: `len` once every queued write whose size is known
/// lands, `written` on disk now. `None` = no such file (a directory too).
pub fn stat(file: &FileRef) -> Option<ClientFileInfo> {
    let known = |store: &Store| {
        store
            .sched
            .files
            .get(&file.key)
            .filter(|state| state.queued() && state.disk != Disk::Unknown)
            .map(|state| state.disk)
    };
    let meta = match shared().with(|store| known(store)) {
        Some(_) => None,
        None => std::fs::metadata(file.path()).ok(),
    };
    shared().with(|store| {
        let known = known(store);
        let base = match known {
            Some(Disk::Len(len)) => Some(len),
            Some(_) => None,
            None => meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
        };
        let state = store.sched.files.get(&file.key);
        let len = match state {
            Some(state) => state.projected(base),
            None => base,
        }?;
        let modified = match known {
            Some(_) => state.and_then(|state| state.modified_ms),
            None => meta.as_ref().map(io::modified_ms),
        };
        Some(ClientFileInfo {
            len,
            written: base.unwrap_or(0),
            modified_unix_ms: modified.unwrap_or(0),
            error: state.and_then(|state| state.error.clone()),
        })
    })
}

/// A record the engine appends to `file` in its turn, though its bytes are
/// still being encoded: [`RecordSlot::fill`] hands them over. `done` answers
/// where the record landed once it is handed to the OS.
pub fn record(
    file: &FileRef,
    done: impl FnOnce(Result<[u64; 2], String>) + Send + 'static,
) -> Result<RecordSlot, String> {
    shared().with(|store| {
        store.in_use(file, false, false)?;
        let job = Job::Put {
            file: file.clone(),
            at: At::End,
            bytes: None,
            record: true,
            poisoned: None,
            done: Box::new(done),
        };
        let op = store.sched.mutate(&file.key, Effect::Grow(None), job, true);
        Ok(RecordSlot { op: Some(op) })
    })
}

/// Bytes a write hands the store: owned, or shared with a cache that keeps
/// them, so a piece is written without a copy either way.
pub enum Chunk {
    Owned(Vec<u8>),
    Shared(Arc<[u8]>),
}

impl std::ops::Deref for Chunk {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Chunk::Owned(bytes) => bytes,
            Chunk::Shared(bytes) => bytes,
        }
    }
}

impl From<Vec<u8>> for Chunk {
    fn from(bytes: Vec<u8>) -> Self {
        Chunk::Owned(bytes)
    }
}

impl From<Arc<[u8]>> for Chunk {
    fn from(bytes: Arc<[u8]>) -> Self {
        Chunk::Shared(bytes)
    }
}

/// A queued record's place in its file; dropped unfilled, it fails. Fill or
/// drop it promptly: everything queued to the file after it waits for it,
/// an instance shutting down included.
pub struct RecordSlot {
    op: Option<queue::OpId>,
}

impl RecordSlot {
    /// The record's bytes, written in one go in these pieces.
    pub fn fill<C: Into<Chunk>>(mut self, pieces: impl IntoIterator<Item = C>) {
        let pieces: Vec<Chunk> = pieces.into_iter().map(Into::into).collect();
        let len = pieces.iter().map(|piece| piece.len() as u64).sum();
        self.settle(Ok(pieces), len);
    }

    /// The record could not be made; its ticket answers `why`.
    pub fn fail(mut self, why: String) {
        self.settle(Err(why), 0);
    }

    fn settle(&mut self, bytes: Result<Vec<Chunk>, String>, len: u64) {
        let Some(op) = self.op.take() else {
            return;
        };
        shared().with(|store| {
            if let Some(Job::Put { bytes: slot, .. }) = store.sched.job_mut(op) {
                *slot = Some(bytes);
            }
            store.sched.release(op, len);
        });
    }
}

impl Drop for RecordSlot {
    fn drop(&mut self) {
        self.settle(Err("the engine abandoned this record".into()), 0);
    }
}

/// While held, `file` is written from outside its queue (see [`WriterKind`]).
pub struct WriterClaim {
    id: u64,
    key: FileKey,
    exclusive: bool,
}

/// Claim `file` for an external writer. `Err` = another writer's claim
/// excludes this one.
pub fn claim(file: &FileRef, kind: WriterKind) -> Result<WriterClaim, String> {
    shared().with(|store| {
        let exclusive = matches!(kind, WriterKind::Exclusive(_));
        if let Some((_, other)) = store.writers.get(&file.key).and_then(|all| {
            all.iter()
                .find(|(_, other)| exclusive || matches!(other, WriterKind::Exclusive(_)))
        }) {
            let (WriterKind::Appends(what) | WriterKind::Exclusive(what)) = *other;
            return Err(format!("{} is being written by {what}", file.rel));
        }
        let id = issue_id();
        store
            .writers
            .entry(file.key.clone())
            .or_default()
            .push((id, kind));
        Ok(WriterClaim {
            id,
            key: file.key.clone(),
            exclusive,
        })
    })
}

impl Drop for WriterClaim {
    fn drop(&mut self) {
        shared().with(|store| {
            if let Some(all) = store.writers.get_mut(&self.key) {
                all.retain(|(id, _)| *id != self.id);
                if all.is_empty() {
                    store.writers.remove(&self.key);
                }
            }
            if self.exclusive {
                // Written behind the store's back: learn it afresh.
                let state = store.sched.file_mut(&self.key);
                state.disk = Disk::Unknown;
                state.handle = None;
            }
        });
    }
}

/// Every [`FileChange`] from now on.
pub fn subscribe() -> mpsc::Receiver<FileChange> {
    let (tx, rx) = mpsc::channel();
    shared().with(|store| store.subscribers.push(tx));
    rx
}

/// Show `file` in the OS file manager. `false` = no such path, or no file
/// manager.
pub fn reveal(file: &FileRef) -> bool {
    reveal::reveal(file.path())
}

/// A scope's files directory, `None` = no such bucket (`World` on the
/// shell) or no folder chosen.
pub(in crate::modding) fn root(
    client: &super::ClientStoreData,
    scope: ClientStorageScope,
) -> Option<Arc<Path>> {
    let bucket = match scope {
        ClientStorageScope::Pack => &client.pack_storage,
        ClientStorageScope::World => client.storage.as_ref()?,
        ClientStorageScope::Chosen(slot) => {
            return folders::chosen(client.pack_storage.dir(), slot).map(Arc::from);
        }
    };
    Some(Arc::from(bucket.dir().join("files")))
}

/// `path` in `scope`'s bucket. The caller has checked the path rule.
pub(in crate::modding) fn locate(
    client: &super::ClientStoreData,
    scope: ClientStorageScope,
    path: &str,
) -> Option<FileRef> {
    root(client, scope).map(|root| FileRef::new(root, path))
}

type FileResult = Result<ClientFileAnswer, String>;

/// One instance's file tickets: issued at the call, answered from any
/// thread, consumed by the poll that reads the answer.
pub(in crate::modding) struct FileTickets {
    tx: mpsc::Sender<(u64, FileResult)>,
    rx: mpsc::Receiver<(u64, FileResult)>,
    waiting: HashSet<u64>,
    ready: HashMap<u64, FileResult>,
    /// Every file this instance mutated: synced when it shuts down.
    touched: BTreeMap<FileKey, FileRef>,
}

impl Default for FileTickets {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            waiting: HashSet::new(),
            ready: HashMap::new(),
            touched: BTreeMap::new(),
        }
    }
}

impl FileTickets {
    /// A new ticket, answered through its [`TicketSink`].
    pub fn issue(&mut self) -> TicketSink {
        let ticket = issue_id();
        self.waiting.insert(ticket);
        TicketSink {
            ticket,
            tx: Some(self.tx.clone()),
        }
    }

    /// A ticket whose operation was refused: never issued after all.
    pub fn withdraw(&mut self, ticket: u64) {
        self.waiting.remove(&ticket);
    }

    /// Remember that this instance writes `file`.
    pub fn touch(&mut self, file: &FileRef) {
        self.touched
            .entry(file.key.clone())
            .or_insert_with(|| file.clone());
    }

    /// `Ok(None)` = not finished; `Ok(Some)` answers and consumes the
    /// ticket; `Err` = never issued to this instance, or already consumed.
    pub fn poll(&mut self, ticket: u64) -> Result<Option<FileResult>, String> {
        while let Ok((done, result)) = self.rx.try_recv() {
            if self.waiting.remove(&done) {
                self.ready.insert(done, result);
            }
        }
        if let Some(result) = self.ready.remove(&ticket) {
            return Ok(Some(result));
        }
        if self.waiting.contains(&ticket) {
            return Ok(None);
        }
        Err(format!(
            "ticket {ticket} was never issued to this instance, or was already polled"
        ))
    }
}

impl Drop for FileTickets {
    /// An instance's files are synced when it shuts down: queued behind what
    /// is written to them, never waited for here. [`flush`] waits, at exit.
    fn drop(&mut self) {
        for file in self.touched.values() {
            queue_sync(file, true, Box::new(|_| {}));
        }
    }
}

/// Block until every queued file operation, and whatever their completions
/// queued in turn, has run. For the process's exit only: the I/O threads die
/// with it.
pub fn flush() {
    let shared = shared();
    let mut store = shared.lock();
    while !store.sched.is_idle() || store.active > 0 {
        store = shared
            .idle
            .wait(store)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Where a ticket's answer goes, from whichever thread finishes it.
pub struct TicketSink {
    ticket: u64,
    tx: Option<mpsc::Sender<(u64, FileResult)>>,
}

impl TicketSink {
    pub fn ticket(&self) -> u64 {
        self.ticket
    }

    pub fn finish(mut self, result: FileResult) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send((self.ticket, result));
        }
    }
}

impl Drop for TicketSink {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send((self.ticket, Err("the engine dropped this operation".into())));
        }
    }
}
