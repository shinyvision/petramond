//! A client mod's FILES in its storage buckets, and the one store every
//! writer goes through: the mod's appends, writes, syncs, renames, deletes,
//! reads, listings, plus the records the engine writes into a mod's files.
//!
//! Mutations of one file land in submission order ([`queue`]); I/O threads
//! ([`io`]) do the blocking calls, so nothing here touches disk on the frame
//! except [`stat`]'s one metadata read of a file the store isn't writing.
//! Sizes aren't counted or capped: a fast producer just grows memory, and
//! [`stat`]'s `len - written` lets the mod track its own backlog.
//!
//! Every file has an INCARNATION: rename moves it, delete ends it, a
//! positioned write announces the bytes it changed ([`subscribe`]). That's
//! what keeps a cache keyed by `(incarnation, offset, len)` from going stale.

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

pub fn issue_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[derive(Clone, Debug)]
pub struct FileRef {
    root: Arc<Path>,
    rel: Arc<str>,
    key: FileKey,
}

impl FileRef {
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

    #[cfg(test)]
    pub fn in_dir(dir: &Path, rel: &str) -> Self {
        Self::new(Arc::from(dir), rel)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn in_bucket(root: &Path, rel: &str) -> Self {
        Self::new(Arc::from(root), rel)
    }

    pub fn rel(&self) -> &str {
        &self.rel
    }

    pub fn path(&self) -> PathBuf {
        paths::join(&self.root, &self.rel)
    }

    pub fn sibling(&self, rel: &str) -> Self {
        Self::new(self.root.clone(), rel)
    }

    fn os(&self, error: impl std::fmt::Display) -> String {
        format!("{}: {error}", self.rel)
    }

    pub fn incarnation(&self) -> u64 {
        shared().with(|store| store.incarnation(&self.key))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    Overwritten {
        incarnation: u64,
        range: [u64; 2],
    },
    Ended {
        incarnations: Vec<u64>,
        path: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterKind {
    Appends(&'static str),
    Exclusive(&'static str),
}

struct Store {
    sched: Sched<Job, Handle>,
    incarnations: BTreeMap<FileKey, u64>,
    next_incarnation: u64,
    writers: BTreeMap<FileKey, Vec<(u64, WriterKind)>>,
    subscribers: Vec<mpsc::Sender<FileChange>>,
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
    idle: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn with<R>(&self, change: impl FnOnce(&mut Store) -> R) -> R {
        let mut store = self.lock();
        let before = store.sched.ready_len();
        let result = change(&mut store);
        for _ in before..store.sched.ready_len() {
            self.work.notify_one();
        }
        result
    }

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

type Handle = (Arc<str>, Arc<File>);

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

pub struct RecordSlot {
    op: Option<queue::OpId>,
}

impl RecordSlot {
    pub fn fill<C: Into<Chunk>>(mut self, pieces: impl IntoIterator<Item = C>) {
        let pieces: Vec<Chunk> = pieces.into_iter().map(Into::into).collect();
        let len = pieces.iter().map(|piece| piece.len() as u64).sum();
        self.settle(Ok(pieces), len);
    }

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

pub struct WriterClaim {
    id: u64,
    key: FileKey,
    exclusive: bool,
}

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
                let state = store.sched.file_mut(&self.key);
                state.disk = Disk::Unknown;
                state.handle = None;
            }
        });
    }
}

pub fn subscribe() -> mpsc::Receiver<FileChange> {
    let (tx, rx) = mpsc::channel();
    shared().with(|store| store.subscribers.push(tx));
    rx
}

pub fn reveal(file: &FileRef) -> bool {
    reveal::reveal(file.path())
}

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

pub(in crate::modding) fn locate(
    client: &super::ClientStoreData,
    scope: ClientStorageScope,
    path: &str,
) -> Option<FileRef> {
    root(client, scope).map(|root| FileRef::new(root, path))
}

type FileResult = Result<ClientFileAnswer, String>;

pub(in crate::modding) struct FileTickets {
    tx: mpsc::Sender<(u64, FileResult)>,
    rx: mpsc::Receiver<(u64, FileResult)>,
    waiting: HashSet<u64>,
    ready: HashMap<u64, FileResult>,
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
    pub fn issue(&mut self) -> TicketSink {
        let ticket = issue_id();
        self.waiting.insert(ticket);
        TicketSink {
            ticket,
            tx: Some(self.tx.clone()),
        }
    }

    pub fn withdraw(&mut self, ticket: u64) {
        self.waiting.remove(&ticket);
    }

    pub fn touch(&mut self, file: &FileRef) {
        self.touched
            .entry(file.key.clone())
            .or_insert_with(|| file.clone());
    }

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
    fn drop(&mut self) {
        for file in self.touched.values() {
            queue_sync(file, true, Box::new(|_| {}));
        }
    }
}

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
