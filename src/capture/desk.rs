//! What the client mods' capture calls and the client session share: the
//! presented world's moment (published by the session each frame), and the
//! events logs (begun and polled by the calls, fed by the session).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::events::{EventsLogs, FrameInput};
use super::moment::Moment;
use crate::worker::JobPool;

#[derive(Default)]
pub struct Desk {
    /// Moves once per presented frame.
    frame: u64,
    /// This frame's world drive has begun: a log begun now takes the next
    /// frame.
    driving: bool,
    /// The presented world's moment as the last frame left it; `None` while
    /// nothing is presented.
    pub moment: Option<Moment>,
    pub logs: EventsLogs,
}

/// One per client runtime, shared by every mod in it and by the session.
#[derive(Clone, Default)]
pub struct CaptureDesk(Arc<Mutex<Desk>>);

impl CaptureDesk {
    pub fn lock(&self) -> MutexGuard<'_, Desk> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Desk {
    /// The first frame an events log begun now takes: this one, unless its
    /// apply already ran.
    pub fn first_frame(&self) -> u64 {
        self.frame + u64::from(self.driving)
    }

    /// The frame's world drive begins: answers whether an events log takes
    /// it, so the session collects what this frame applies.
    pub fn begin_frame(&mut self) -> bool {
        self.driving = true;
        self.logs.running()
    }

    /// The frame is presented: its record goes to every running log that
    /// began before its world drive.
    pub fn submit_frame(&mut self, input: FrameInput, jobs: &Arc<JobPool>) {
        let frame = self.frame;
        self.logs.submit(frame, input, jobs);
    }

    /// The presented frame is over.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        self.driving = false;
    }
}
