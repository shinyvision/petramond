//! The file I/O threads: the blocking system calls (write, pread, fsync,
//! rename, unlink, readdir), never on the frame and never on the job pool, so
//! a stalled disk delays neither a frame nor meshing. There are
//! `available_parallelism()` of them, a pipeline depth, not a limit: they
//! take ready work from every file in the order it became ready.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mod_api::{ClientFileAnswer, ClientFileEntry, HostRet};

use super::queue::{Disk, OpId};
use super::{now_ms, paths, shared, Chunk, FileRef};

type Hook<T> = Box<dyn FnOnce(Result<T, String>) + Send>;

pub type Listing = (Vec<ClientFileEntry>, bool);

pub(super) enum At {
    End,
    Offset { offset: u64, truncate: bool },
}

pub(super) enum Job {
    Put {
        file: FileRef,
        at: At,
        bytes: Option<Result<Vec<Chunk>, String>>,
        record: bool,
        poisoned: Option<String>,
        done: Hook<[u64; 2]>,
    },
    Sync {
        file: FileRef,
        if_dirty: bool,
        done: Hook<()>,
    },
    Rename {
        from: FileRef,
        to: FileRef,
        done: Hook<()>,
    },
    Delete {
        file: FileRef,
        done: Hook<()>,
    },
    Read {
        file: FileRef,
        offset: u64,
        len: u64,
        done: Hook<Vec<u8>>,
    },
    List {
        dir: FileRef,
        after: Option<String>,
        max_bytes: u64,
        done: Hook<Listing>,
    },
    Unlink(PathBuf),
}

impl Job {
    pub(super) fn fail(self, why: String) {
        match self {
            Job::Put { done, .. } => done(Err(why)),
            Job::Sync { done, .. } | Job::Rename { done, .. } | Job::Delete { done, .. } => {
                done(Err(why))
            }
            Job::Read { done, .. } => done(Err(why)),
            Job::List { done, .. } => done(Err(why)),
            Job::Unlink(_) => {}
        }
    }
}

pub(super) fn spawn_threads() {
    let threads = std::thread::available_parallelism().map_or(2, usize::from);
    for n in 0..threads {
        std::thread::Builder::new()
            .name(format!("client-mod-files-{n}"))
            .spawn(worker)
            .expect("spawn a client mod file thread");
    }
}

fn worker() {
    let shared = shared();
    loop {
        let (id, job) = {
            let mut store = shared.lock();
            loop {
                if let Some(next) = store.sched.take_ready() {
                    store.active += 1;
                    break next;
                }
                store = shared
                    .work
                    .wait(store)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        run(id, job);
        let mut store = shared.lock();
        store.active -= 1;
        if store.active == 0 && store.sched.is_idle() {
            shared.idle.notify_all();
        }
    }
}

fn run(id: OpId, job: Job) {
    match job {
        Job::Put {
            file,
            at,
            bytes,
            record,
            poisoned,
            done,
        } => put(
            id,
            &file,
            at,
            bytes.map(|b| poisoned.map_or(b, Err)),
            record,
            done,
        ),
        Job::Sync {
            file,
            if_dirty,
            done,
        } => {
            let result = sync(id, &file, if_dirty);
            done(result)
        }
        Job::Rename { from, to, done } => {
            let result = rename(id, &from, &to);
            done(result)
        }
        Job::Delete { file, done } => {
            let result = delete(id, &file);
            done(result)
        }
        Job::Read {
            file,
            offset,
            len,
            done,
        } => {
            let result = read(&file, offset, len);
            shared().finish(id);
            done(result)
        }
        Job::List {
            dir,
            after,
            max_bytes,
            done,
        } => {
            let result = list(&dir, after.as_deref(), max_bytes);
            shared().finish(id);
            done(result)
        }
        Job::Unlink(trash) => {
            let removed = if trash.is_dir() {
                std::fs::remove_dir_all(&trash)
            } else {
                std::fs::remove_file(&trash)
            };
            if let Err(error) = removed {
                log::warn!("remove deleted mod files {}: {error}", trash.display());
            }
            shared().finish(id);
        }
    }
}

fn put(
    id: OpId,
    file: &FileRef,
    at: At,
    bytes: Option<Result<Vec<Chunk>, String>>,
    record: bool,
    done: Hook<[u64; 2]>,
) {
    let open = shared().with(|store| {
        let state = store.sched.file_mut(&file.key);
        match (&state.handle, state.disk) {
            (Some((rel, handle)), Disk::Len(len)) if *rel == file.rel => {
                Some((handle.clone(), len))
            }
            _ => None,
        }
    });
    let at_end = matches!(at, At::End);
    let mut began: Option<(Arc<File>, u64)> = None;
    let result = (|| -> Result<([u64; 2], Arc<File>, u64), String> {
        let chunks = bytes.unwrap_or_else(|| Err("the engine never wrote this record".into()))?;
        let (handle, len) = match open {
            Some(open) => open,
            None => {
                let handle = paths::create(&file.root, &file.rel, |path| {
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(path)
                })?;
                let len = handle.metadata().map_err(|e| file.os(e))?.len();
                (Arc::new(handle), len)
            }
        };
        let offset = match at {
            At::End => len,
            At::Offset { offset, .. } => offset,
        };
        if at_end {
            began = Some((handle.clone(), offset));
        }
        let mut end = offset;
        for chunk in &chunks {
            write_all_at(&handle, chunk, end).map_err(|e| file.os(e))?;
            end += chunk.len() as u64;
        }
        let new_len = match at {
            At::Offset { truncate: true, .. } => {
                handle.set_len(end).map_err(|e| file.os(e))?;
                end
            }
            _ => len.max(end),
        };
        Ok(([offset, end - offset], handle, new_len))
    })();
    let (range, record_state) = match result {
        Ok((range, handle, len)) => (Ok(range), Ok((handle, len))),
        Err(why) => {
            let cut = began.filter(|(handle, offset)| handle.set_len(*offset).is_ok());
            (Err(why.clone()), Err((why, cut)))
        }
    };
    let done = match &range {
        Err(_) => {
            done(range.clone());
            None
        }
        Ok(_) => Some(done),
    };
    shared().landed(id, |store| match &record_state {
        Ok((handle, len)) => {
            let state = store.sched.file_mut(&file.key);
            state.handle = Some((file.rel.clone(), handle.clone()));
            state.disk = Disk::Len(*len);
            state.modified_ms = Some(now_ms());
            state.error = None;
            state.dirty = true;
        }
        Err((why, cut)) => {
            if record {
                for behind in store.sched.queued_behind(&file.key, id) {
                    if let Some(Job::Put {
                        record: true,
                        poisoned,
                        ..
                    }) = store.sched.job_mut(behind)
                    {
                        poisoned.get_or_insert_with(|| {
                            format!("an earlier record of {} failed: {why}", file.rel)
                        });
                    }
                }
            }
            let state = store.sched.file_mut(&file.key);
            match cut {
                Some((handle, len)) => {
                    state.handle = Some((file.rel.clone(), handle.clone()));
                    state.disk = Disk::Len(*len);
                }
                None => {
                    state.handle = None;
                    state.disk = Disk::Unknown;
                }
            }
            state.error = Some(why.clone());
        }
    });
    if let Some(done) = done {
        done(range);
    }
}

fn sync(id: OpId, file: &FileRef, if_dirty: bool) -> Result<(), String> {
    let (handle, dirty) = shared().with(|store| {
        let state = store.sched.file_mut(&file.key);
        let handle = state.handle.as_ref().filter(|(rel, _)| *rel == file.rel);
        (handle.map(|(_, handle)| handle.clone()), state.dirty)
    });
    let result = if if_dirty && !dirty {
        Ok(())
    } else {
        let handle = match handle {
            Some(handle) => Ok(handle),
            None => OpenOptions::new()
                .write(true)
                .open(file.path())
                .map(Arc::new)
                .map_err(|e| file.os(e)),
        };
        handle.and_then(|handle| handle.sync_all().map_err(|e| file.os(e)))
    };
    shared().landed(id, |store| {
        if result.is_ok() {
            store.sched.file_mut(&file.key).dirty = false;
        }
    });
    result
}

fn rename(id: OpId, from: &FileRef, to: &FileRef) -> Result<(), String> {
    let (from_path, to_path) = (from.path(), to.path());
    let result = (|| -> Result<(), String> {
        let meta = std::fs::metadata(&from_path).map_err(|e| from.os(e))?;
        if std::fs::metadata(&to_path).is_ok_and(|m| m.is_dir()) {
            return Err(format!("{} is a directory", to.rel));
        }
        if meta.is_file() {
            OpenOptions::new()
                .write(true)
                .open(&from_path)
                .and_then(|handle| handle.sync_all())
                .map_err(|e| from.os(e))?;
        }
        paths::create(&to.root, &to.rel, |path| std::fs::rename(&from_path, path))?;
        if from_path != to_path {
            paths::forget(&from_path);
        }
        sync_dir(&to_path);
        if from_path.parent() != to_path.parent() {
            sync_dir(&from_path);
        }
        Ok(())
    })();
    shared().landed(id, |store| {
        for key in store.sched.subtree(&from.key) {
            let state = store.sched.file_mut(&key);
            state.handle = None;
            state.disk = Disk::Absent;
            state.dirty = false;
        }
        for key in store.sched.subtree(&to.key) {
            let state = store.sched.file_mut(&key);
            state.handle = None;
            state.disk = Disk::Unknown;
        }
    });
    result
}

static TRASH: AtomicU64 = AtomicU64::new(0);

fn delete(id: OpId, file: &FileRef) -> Result<(), String> {
    let path = file.path();
    let trash = path.with_file_name(format!(
        ".trash-{}-{}",
        std::process::id(),
        TRASH.fetch_add(1, Ordering::Relaxed)
    ));
    let result = std::fs::rename(&path, &trash).map_err(|e| file.os(e));
    if result.is_ok() {
        paths::forget(&path);
    }
    shared().landed(id, |store| {
        for key in store.sched.subtree(&file.key) {
            let state = store.sched.file_mut(&key);
            state.handle = None;
            state.disk = Disk::Absent;
            state.dirty = false;
        }
        if result.is_ok() {
            store.sched.detached(Job::Unlink(trash));
        }
    });
    result
}

fn read(file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>, String> {
    let open = shared().with(|store| {
        store
            .sched
            .files
            .get(&file.key)
            .and_then(|state| state.handle.as_ref())
            .filter(|(rel, _)| *rel == file.rel)
            .map(|(_, handle)| handle.clone())
    });
    let handle = match open {
        Some(handle) => handle,
        None => Arc::new(File::open(file.path()).map_err(|e| file.os(e))?),
    };
    let size = handle.metadata().map_err(|e| file.os(e))?.len();
    let take = len.min(size.saturating_sub(offset));
    let mut bytes = vec![0; usize::try_from(take).map_err(|_| file.os("too large to read"))?];
    read_exact_at(&handle, &mut bytes, offset).map_err(|e| file.os(e))?;
    Ok(bytes)
}

fn list(dir: &FileRef, after: Option<&str>, max_bytes: u64) -> Result<Listing, String> {
    let path = dir.path();
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) if dir.rel.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), false))
        }
        Err(error) => return Err(dir.os(error)),
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && after.is_none_or(|after| name.as_str() > after))
        .collect();
    names.sort_unstable();

    let empty = HostRet::ClientFilePolled(Some(ClientFileAnswer::Listing {
        entries: Vec::new(),
        more: true,
    }));
    let base = mod_api::encode(&empty).map_or(0, |bytes| bytes.len() as u64) - 1;
    let mut entries_len = 0;
    let mut page = Vec::new();
    for name in names {
        let meta = std::fs::metadata(path.join(&name)).ok();
        let entry = ClientFileEntry {
            dir: meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
            len: meta.as_ref().filter(|m| m.is_file()).map_or(0, |m| m.len()),
            modified_unix_ms: meta.as_ref().map_or(0, modified_ms),
            name,
        };
        let entry_len = mod_api::encode(&entry).map_or(0, |bytes| bytes.len() as u64);
        let answer_len = base + varint_len(page.len() as u64 + 1) + entries_len + entry_len;
        if !page.is_empty() && answer_len > max_bytes {
            return Ok((page, true));
        }
        entries_len += entry_len;
        page.push(entry);
    }
    Ok((page, false))
}

fn varint_len(n: u64) -> u64 {
    u64::from((64 - n.leading_zeros()).max(1).div_ceil(7))
}

pub(super) fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as u64)
}

fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(dir) = File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(unix)]
fn write_all_at(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, bytes, offset)
}

#[cfg(unix)]
fn read_exact_at(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut bytes: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !bytes.is_empty() {
        match file.seek_write(bytes, offset) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                bytes = &bytes[n..];
                offset += n as u64;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !bytes.is_empty() {
        match file.seek_read(bytes, offset) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                bytes = &mut bytes[n..];
                offset += n as u64;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
