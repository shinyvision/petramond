//! The presentation desk: where a client mod's presentation calls meet the
//! client presenting the world. A call validates, answers from what the
//! calls so far leave the presentation at, and queues a request; the client
//! carries the requests out, in issue order, at its next frame and publishes
//! where the presentation stands.
//!
//! One desk per runtime, inside [`super::presented::Presented`], shared by
//! every instance: only the mod that opened the presentation (its OWNER)
//! drives it.

use std::collections::{BTreeMap, VecDeque};

use mod_api::{ClientPose, ClientPresentationStateData};

use crate::capture::present::Op;
use crate::capture::source::FileRanges;

/// What the client is asked to do, in issue order.
#[derive(Clone, Debug)]
pub enum Request {
    Open {
        owner: String,
        tables: FileRanges,
        seed: u32,
        mods: Vec<String>,
        viewer: Option<ClientPose>,
    },
    Op(Op),
    Cancel(u64),
    Viewer {
        pose: ClientPose,
        flying: bool,
    },
    Close,
}

/// Where the client says the presentation stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Published {
    pub opening: bool,
    pub open: bool,
    pub position: f64,
    pub released_through: Option<u64>,
    pub ready: bool,
    pub applied: u64,
    pub exhausted: bool,
    pub error: Option<String>,
}

/// A call that moves the position, while the client may not have carried it
/// out yet.
#[derive(Clone, Copy, Debug)]
enum Step {
    Apply(u64, f64),
    Time(f64),
}

#[derive(Default)]
pub struct PresentationDesk {
    /// The mod whose presentation is open or opening.
    owner: Option<String>,
    requests: VecDeque<Request>,
    /// Applies issued and not yet landed, failed or cancelled, in order.
    pending: Vec<u64>,
    /// Every apply id this desk issued, with the mod it was issued to.
    issued: BTreeMap<u64, String>,
    /// The position the client last published with nothing of the calls'
    /// left to carry out, and the calls since: the position they leave is
    /// replayed from both, so a cancelled or failed apply takes its effect
    /// with it.
    base: f64,
    steps: Vec<Step>,
    published: Published,
    /// Why the last refused open was refused, until the next open.
    refusal: Option<String>,
}

impl PresentationDesk {
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// Whether `mod_id` owns the presentation open (or opening).
    pub fn owns(&self, mod_id: &str) -> bool {
        self.owner.as_deref() == Some(mod_id)
    }

    /// An open was refused for `why`.
    pub fn refuse(&mut self, why: String) {
        self.refusal = Some(why);
    }

    /// `mod_id` opens a presentation; one it already owns is replaced.
    pub fn open(
        &mut self,
        mod_id: &str,
        tables: FileRanges,
        seed: u32,
        mods: Vec<String>,
        viewer: Option<ClientPose>,
    ) {
        self.owner = Some(mod_id.to_owned());
        self.refusal = None;
        self.pending.clear();
        self.base = 0.0;
        self.steps.clear();
        self.published = Published {
            opening: true,
            ..Default::default()
        };
        self.requests.push_back(Request::Open {
            owner: mod_id.to_owned(),
            tables,
            seed,
            mods,
            viewer,
        });
    }

    pub fn apply(
        &mut self,
        id: u64,
        mod_id: &str,
        state: Vec<FileRanges>,
        events: Vec<FileRanges>,
        at: f64,
    ) {
        self.issued.insert(id, mod_id.to_owned());
        self.pending.push(id);
        self.steps.push(Step::Apply(id, at));
        self.requests.push_back(Request::Op(Op::Apply {
            id,
            state,
            events,
            at,
        }));
    }

    /// `Some(true)`: apply `id` will never land. `Some(false)`: it already
    /// did (or failed). `None`: never issued to `mod_id`.
    pub fn cancel(&mut self, id: u64, mod_id: &str) -> Option<bool> {
        if self.issued.get(&id).map(String::as_str) != Some(mod_id) {
            return None;
        }
        let Some(i) = self.pending.iter().position(|&p| p == id) else {
            return Some(false);
        };
        self.pending.remove(i);
        self.steps
            .retain(|step| !matches!(step, Step::Apply(x, _) if *x == id));
        let queued = self
            .requests
            .iter()
            .position(|r| matches!(r, Request::Op(Op::Apply { id: x, .. }) if *x == id));
        match queued {
            Some(q) => {
                self.requests.remove(q);
            }
            None => self.requests.push_back(Request::Cancel(id)),
        }
        Some(true)
    }

    pub fn queue(&mut self, events: Vec<FileRanges>) {
        self.requests.push_back(Request::Op(Op::Queue(events)));
    }

    /// `false`: back past the committed pair, which only an apply reaches.
    pub fn time(&mut self, at: f64) -> bool {
        if !time_reaches(self.logical(), at) {
            return false;
        }
        self.steps.push(Step::Time(at));
        self.requests.push_back(Request::Op(Op::Time(at)));
        true
    }

    /// The position the calls still standing leave, as the client carries
    /// them out.
    fn logical(&self) -> f64 {
        self.steps.iter().fold(self.base, |at, step| match *step {
            Step::Apply(_, to) => to,
            Step::Time(to) if time_reaches(at, to) => to,
            Step::Time(_) => at,
        })
    }

    pub fn viewer(&mut self, pose: ClientPose, flying: bool) {
        self.requests.push_back(Request::Viewer { pose, flying });
    }

    pub fn close(&mut self) {
        self.owner = None;
        self.pending.clear();
        self.steps.clear();
        self.published = Published::default();
        self.requests.push_back(Request::Close);
    }

    pub fn state(&self) -> ClientPresentationStateData {
        let p = &self.published;
        ClientPresentationStateData {
            opening: self.owner.is_some() && p.opening,
            open: self.owner.is_some() && p.open,
            owner: self.owner.clone(),
            position: p.position,
            released_through: p.released_through,
            ready: p.ready && self.pending.is_empty(),
            pending: self.pending.clone(),
            applied: p.applied,
            exhausted: p.exhausted,
            error: p.error.clone().or_else(|| self.refusal.clone()),
        }
    }

    // --- the client's side ---

    /// The newest open request, with every request before it (on the shell
    /// nothing is open for them to act on); what follows it stays queued
    /// for the presentation the open builds.
    pub fn take_open(&mut self) -> Option<Request> {
        let last = self
            .requests
            .iter()
            .rposition(|r| matches!(r, Request::Open { .. }))?;
        self.requests.drain(..=last).next_back()
    }

    /// Every request since the last frame, in issue order.
    pub fn take_requests(&mut self) -> Vec<Request> {
        self.requests.drain(..).collect()
    }

    /// Where the presentation stands, with the applies that landed and
    /// those that failed since the last publish.
    pub fn publish(&mut self, state: Published, landed: &[u64], failed: &[u64]) {
        self.pending
            .retain(|id| !landed.contains(id) && !failed.contains(id));
        self.steps
            .retain(|step| !matches!(step, Step::Apply(id, _) if failed.contains(id)));
        let settled =
            self.pending.is_empty() && !self.requests.iter().any(|r| matches!(r, Request::Op(_)));
        if settled {
            self.base = state.position;
            self.steps.clear();
        }
        self.published = state;
    }

    /// The presentation ended without its owner asking (it never opened, or
    /// its owner stopped running): nothing is open any more.
    pub fn ended(&mut self, why: Option<String>) {
        self.owner = None;
        self.pending.clear();
        self.steps.clear();
        self.published = Published {
            error: why,
            ..Default::default()
        };
    }
}

/// Whether `Time` may move the position from `from` to `to`: forward, or
/// back only inside the committed pair.
pub fn time_reaches(from: f64, to: f64) -> bool {
    to >= from || to.floor() == from.floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An apply that is cancelled or fails takes its position with it: a
    /// `Time` is checked against where the calls still standing leave it.
    #[test]
    fn a_time_is_checked_against_the_applies_still_standing() {
        let mut desk = PresentationDesk::default();
        desk.apply(1, "m", Vec::new(), Vec::new(), 1000.0);
        desk.take_requests();
        let landed = Published {
            open: true,
            position: 1000.0,
            applied: 1,
            ..Default::default()
        };
        desk.publish(landed.clone(), &[1], &[]);

        desk.apply(2, "m", Vec::new(), Vec::new(), 50.0);
        assert_eq!(desk.cancel(2, "m"), Some(true));
        assert!(!desk.time(60.0), "the cancelled apply's position is gone");

        desk.apply(3, "m", Vec::new(), Vec::new(), 50.0);
        desk.take_requests();
        desk.publish(landed, &[], &[3]);
        assert!(!desk.time(60.0), "and so is a failed one's");
        assert!(desk.time(1000.5));
    }
}
