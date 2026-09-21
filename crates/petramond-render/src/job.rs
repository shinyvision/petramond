//! A value being computed on the shared background pool, polled from the
//! frame thread.

use petramond::worker::JobPool;
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};

/// Ahead of every streaming stage (their keys are squared distances, never
/// negative): this work answers something the player just did or is looking
/// at, and there is little of it.
const PRESENTATION_KEY: i64 = -1;

/// The job ended without a value: it panicked, or its pool shut down first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLost;

pub struct Job<T> {
    result: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(pool: &JobPool, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, result) = sync_channel(1);
        pool.submit(PRESENTATION_KEY, move || {
            let _ = tx.send(work());
        });
        Self { result }
    }

    /// `None` while the job is still running.
    pub fn poll(&self) -> Option<Result<T, JobLost>> {
        match self.result.try_recv() {
            Ok(value) => Some(Ok(value)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(JobLost)),
        }
    }

    /// Empty `slot` and return its outcome once the job in it has ended.
    pub fn finish(slot: &mut Option<Self>) -> Option<Result<T, JobLost>> {
        let outcome = slot.as_ref()?.poll()?;
        *slot = None;
        Some(outcome)
    }
}

#[cfg(test)]
mod tests;
