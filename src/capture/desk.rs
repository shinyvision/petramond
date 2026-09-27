use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::events::{EventsLogs, FrameInput};
use super::moment::Moment;
use crate::worker::JobPool;

#[derive(Default)]
pub struct Desk {
    frame: u64,
    driving: bool,
    pub moment: Option<Moment>,
    pub logs: EventsLogs,
}

#[derive(Clone, Default)]
pub struct CaptureDesk(Arc<Mutex<Desk>>);

impl CaptureDesk {
    pub fn lock(&self) -> MutexGuard<'_, Desk> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Desk {
    pub fn first_frame(&self) -> u64 {
        self.frame + u64::from(self.driving)
    }

    pub fn begin_frame(&mut self) -> bool {
        self.driving = true;
        self.logs.running()
    }

    pub fn submit_frame(&mut self, input: FrameInput, jobs: &Arc<JobPool>) {
        let frame = self.frame;
        self.logs.submit(frame, input, jobs);
    }

    pub fn end_frame(&mut self) {
        self.frame += 1;
        self.driving = false;
    }
}
