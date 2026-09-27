use petramond::worker::JobPool;
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};

const PRESENTATION_KEY: i64 = -1;

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

    pub fn poll(&self) -> Option<Result<T, JobLost>> {
        match self.result.try_recv() {
            Ok(value) => Some(Ok(value)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(JobLost)),
        }
    }

    pub fn finish(slot: &mut Option<Self>) -> Option<Result<T, JobLost>> {
        let outcome = slot.as_ref()?.poll()?;
        *slot = None;
        Some(outcome)
    }
}

#[cfg(test)]
mod tests;
