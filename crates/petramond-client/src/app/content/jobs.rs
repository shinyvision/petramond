//! The download queue: one download at a time, first in first out, each on
//! its own worker thread. A job downloads the archive into the installed
//! root's hidden `.staging/`, then unpacks, admits and stages it as a pending
//! install; nothing it writes is ever seen by discovery before a restart.
//!
//! A busy site (429 `too_many_downloads`) is never an error: the job waits
//! out `Retry-After` and tries again by itself. Cancel works throughout: the
//! worker checks its flag between reads and is woken from a wait.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use petramond::content::api::{self, DownloadError, Progress};
use petramond::content::install::{self, Offer};
use petramond::content::{Dirs, ListingRow};
use petramond::service::ServiceError;

/// Where a job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Phase {
    Queued,
    Downloading,
    /// petramond.com is busy: trying again at `until`.
    Waiting {
        until: Instant,
    },
    /// Unpacking and checking the archive.
    Checking,
}

pub(in crate::app) struct Job {
    pub(in crate::app) row: ListingRow,
    pub(in crate::app) phase: Phase,
    pub(in crate::app) progress: Arc<Progress>,
    serial: u64,
    cancel: Arc<AtomicBool>,
    /// The worker, woken on cancel so a wait ends at once.
    thread: Option<std::thread::Thread>,
    dirs: Dirs,
    shipped: BTreeSet<String>,
}

impl Job {
    /// Bytes done and expected (the listing's size until the answer says).
    pub(in crate::app) fn bytes(&self) -> (u64, u64) {
        let total = self.progress.total.load(Ordering::Relaxed);
        let total = if total == 0 {
            self.row.byte_size
        } else {
            total
        };
        (self.progress.done.load(Ordering::Relaxed).min(total), total)
    }
}

/// What the queue tells the session.
#[derive(Debug)]
pub(in crate::app) enum Event {
    /// A pending install was written for this pack id.
    Staged(String),
    /// The pack id's download failed, with the reason the row shows.
    Failed(String, String),
    /// petramond.com no longer has the pack (404).
    Gone(String),
    /// A download re-fetched the listing because its package changed.
    Listing(Vec<ListingRow>),
    /// The stored sign-in is dead.
    SignedOut,
}

enum Message {
    Phase(Phase),
    Listing(Vec<ListingRow>),
    Done(Outcome),
}

enum Outcome {
    /// A pending install was written for this pack id.
    Staged(String),
    Failed(String),
    Gone,
    SignedOut,
    Cancelled,
}

pub(in crate::app) struct Jobs {
    queue: VecDeque<Job>,
    tx: Sender<(u64, Message)>,
    rx: Receiver<(u64, Message)>,
    next_serial: u64,
    /// Workers run only outside tests.
    network: bool,
}

impl Jobs {
    pub(super) fn new(network: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            queue: VecDeque::new(),
            tx,
            rx,
            next_serial: 0,
            network,
        }
    }

    pub(in crate::app) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(in crate::app) fn len(&self) -> usize {
        self.queue.len()
    }

    pub(in crate::app) fn get(&self, mod_id: &str) -> Option<&Job> {
        self.queue.iter().find(|j| j.row.mod_id == mod_id)
    }

    pub(in crate::app) fn iter(&self) -> impl Iterator<Item = &Job> {
        self.queue.iter()
    }

    pub(super) fn enqueue(&mut self, row: ListingRow, dirs: Dirs, shipped: BTreeSet<String>) {
        self.next_serial += 1;
        self.queue.push_back(Job {
            row,
            phase: Phase::Queued,
            progress: Arc::new(Progress::default()),
            serial: self.next_serial,
            cancel: Arc::new(AtomicBool::new(false)),
            thread: None,
            dirs,
            shipped,
        });
        self.start_next();
    }

    /// Cancel `mod_id`'s job. Its worker removes its own files; the next job
    /// starts at once.
    pub(in crate::app) fn cancel(&mut self, mod_id: &str) {
        if let Some(i) = self.queue.iter().position(|j| j.row.mod_id == mod_id) {
            if let Some(job) = self.queue.remove(i) {
                stop(&job);
            }
        }
        self.start_next();
    }

    pub(in crate::app) fn cancel_all(&mut self) {
        for job in self.queue.drain(..) {
            stop(&job);
        }
    }

    /// Start the front job if nothing runs.
    fn start_next(&mut self) {
        let Some(job) = self.queue.front_mut() else {
            return;
        };
        if job.phase != Phase::Queued || !self.network {
            return;
        }
        job.phase = Phase::Downloading;
        let tx = self.tx.clone();
        let serial = job.serial;
        let row = job.row.clone();
        let (dirs, shipped) = (job.dirs.clone(), job.shipped.clone());
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("petramond-content-download".to_owned())
            .spawn(move || {
                let report = |message| {
                    let _ = tx.send((serial, message));
                };
                let outcome = run(row, &dirs, &shipped, &progress, &cancel, &report);
                report(Message::Done(outcome));
            });
        match spawned {
            Ok(handle) => job.thread = Some(handle.thread().clone()),
            Err(e) => {
                let _ = self.tx.send((
                    serial,
                    Message::Done(Outcome::Failed(format!("could not start: {e}"))),
                ));
            }
        }
    }

    /// Drain the workers' messages.
    pub(super) fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok((serial, message)) = self.rx.try_recv() {
            let Some(i) = self.queue.iter().position(|j| j.serial == serial) else {
                // A cancelled job still reporting: its listing is still news,
                // and a stage it finished anyway is a real pending install.
                match message {
                    Message::Listing(rows) => events.push(Event::Listing(rows)),
                    Message::Done(Outcome::Staged(id)) => events.push(Event::Staged(id)),
                    _ => {}
                }
                continue;
            };
            match message {
                Message::Phase(phase) => self.queue[i].phase = phase,
                Message::Listing(rows) => events.push(Event::Listing(rows)),
                Message::Done(outcome) => {
                    let Some(job) = self.queue.remove(i) else {
                        continue;
                    };
                    let id = job.row.mod_id;
                    match outcome {
                        Outcome::Staged(_) => events.push(Event::Staged(id)),
                        Outcome::Failed(why) => events.push(Event::Failed(id, why)),
                        Outcome::Gone => events.push(Event::Gone(id)),
                        Outcome::SignedOut => events.push(Event::SignedOut),
                        Outcome::Cancelled => {}
                    }
                    self.start_next();
                }
            }
        }
        events
    }

    /// Put `mod_id`'s job in `phase` (tests arrange what a worker would).
    #[cfg(test)]
    pub(in crate::app) fn set_phase(&mut self, mod_id: &str, phase: Phase, done: u64) {
        if let Some(job) = self.queue.iter_mut().find(|j| j.row.mod_id == mod_id) {
            job.phase = phase;
            job.progress.done.store(done, Ordering::Relaxed);
        }
    }
}

fn stop(job: &Job) {
    job.cancel.store(true, Ordering::Relaxed);
    if let Some(thread) = &job.thread {
        thread.unpark();
    }
}

/// A partial download's path: hidden, and unique for this process's life.
fn partial_path(dirs: &Dirs, mod_id: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    dirs.staging().join(format!(
        "{mod_id}-{:x}{n:x}.zip.partial",
        std::process::id()
    ))
}

/// One job, start to staged. Blocks: the worker's whole life.
fn run(
    mut row: ListingRow,
    dirs: &Dirs,
    shipped: &BTreeSet<String>,
    progress: &Progress,
    cancel: &AtomicBool,
    report: &dyn Fn(Message),
) -> Outcome {
    let mut refetched = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Cancelled;
        }
        report(Message::Phase(Phase::Downloading));
        let partial = partial_path(dirs, &row.mod_id);
        match api::download(&row, &partial, progress, cancel) {
            Ok(zip) => {
                report(Message::Phase(Phase::Checking));
                return match install::stage_install(dirs, &zip, &Offer::from(&row), shipped, cancel)
                {
                    Ok(()) => Outcome::Staged(row.mod_id),
                    Err(_) if cancel.load(Ordering::Relaxed) => Outcome::Cancelled,
                    Err(why) => Outcome::Failed(why),
                };
            }
            Err(DownloadError::Service(ServiceError::Busy { retry_after })) => {
                let until = Instant::now() + retry_after;
                report(Message::Phase(Phase::Waiting { until }));
                wait_until(until, cancel);
            }
            Err(DownloadError::Service(ServiceError::SignInRequired(_))) => {
                return Outcome::SignedOut
            }
            Err(DownloadError::Changed) if !refetched => {
                // The package was replaced after the listing was fetched:
                // look once more, and follow the new row if it moved.
                refetched = true;
                match api::listing() {
                    Ok(rows) => {
                        let fresh = rows.iter().find(|r| r.mod_id == row.mod_id).cloned();
                        report(Message::Listing(rows));
                        match fresh {
                            Some(fresh)
                                if fresh.sha256 != row.sha256
                                    || fresh.byte_size != row.byte_size =>
                            {
                                row = fresh
                            }
                            _ => return Outcome::Failed(DownloadError::Changed.message()),
                        }
                    }
                    Err(ServiceError::SignInRequired(_)) => return Outcome::SignedOut,
                    Err(_) => return Outcome::Failed(DownloadError::Changed.message()),
                }
            }
            Err(DownloadError::Gone) => return Outcome::Gone,
            Err(DownloadError::Cancelled) => return Outcome::Cancelled,
            Err(e) => return Outcome::Failed(e.message()),
        }
    }
}

/// Sleep until `until`, or until cancelled (the canceller unparks us).
fn wait_until(until: Instant, cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        let left = until.saturating_duration_since(Instant::now());
        if left == Duration::ZERO {
            return;
        }
        std::thread::park_timeout(left);
    }
}
