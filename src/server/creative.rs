//! Operator editing: the ordered request queue each session submits edits
//! through, and the stage that works it off one request per tick.

mod edits;
pub mod history;
mod transfer;

pub use history::EditHistory;

use super::game::ServerGame;
use crate::{
    events::tick::TickEvents,
    schematic::{store::Digest, CreativeAction, CreativeReply},
};
use history::Replay;
use std::collections::VecDeque;

/// Requests one session may have waiting. A paste holds its place from its
/// request, so a queued edit cannot overtake a design still arriving.
const QUEUE_DEPTH: usize = 4;
/// Refusals kept for a client that floods a full queue.
const BUSY_REPLIES: usize = 8;

/// A finished capture: its wire archive and the digest naming it.
type Captured = Result<(Digest, std::sync::Arc<[u8]>), String>;

#[derive(Default)]
pub struct CreativeSession {
    pub(super) pending: VecDeque<Pending>,
    pub replies: Vec<CreativeReply>,
    job: Option<edits::EditJob>,
    capture: Option<std::thread::JoinHandle<Captured>>,
}

pub(super) enum Pending {
    Action(CreativeAction),
    Placement {
        design: transfer::Design,
        origin: [i32; 3],
        turns: u8,
    },
}

impl CreativeSession {
    pub(crate) fn take_replies(&mut self) -> Vec<CreativeReply> {
        std::mem::take(&mut self.replies)
    }

    fn has_room(&self) -> bool {
        self.pending.len() < QUEUE_DEPTH
    }

    /// Queue a request behind the ones already waiting; a full queue refuses
    /// it and tells the client.
    pub(super) fn try_enqueue(&mut self, request: Pending) -> bool {
        if self.has_room() {
            self.pending.push_back(request);
            return true;
        }
        if self.replies.len() < BUSY_REPLIES {
            self.refuse("Creative edit queue is busy; try again shortly".into());
        }
        false
    }

    fn refuse(&mut self, message: String) {
        self.replies.push(CreativeReply::Message(message));
    }
}

impl ServerGame {
    /// Whether session `s` may change the world through operator edits.
    pub(super) fn may_edit(&self, s: usize) -> bool {
        let player = &self.sessions[s].player;
        self.is_operator(s) && player.abilities().edits_cells && player.health() > 0
    }

    pub(super) fn tick_creative(&mut self, s: usize, events: &mut TickEvents) {
        if !self.poll_capture(s) {
            return;
        }
        // An edit in progress holds the queue: requests apply in order.
        if self.sessions[s].creative.job.is_some() {
            if let Err(message) = self.step_edit_job(s, events) {
                self.sessions[s].creative.refuse(message);
            }
            return;
        }
        let Some(mut request) = self.sessions[s].creative.pending.pop_front() else {
            return;
        };
        let mut decoded = None;
        if let Pending::Placement { design, .. } = &mut request {
            match self.poll_design(s, design) {
                None => return self.sessions[s].creative.pending.push_front(request),
                Some(Ok(design)) => decoded = Some(design),
                Some(Err(message)) => return self.sessions[s].creative.refuse(message),
            }
        }
        // Saving a selection is authoring, not editing: a player choosing a
        // design for a mod may save one in any mode.
        let authoring = matches!(request, Pending::Action(CreativeAction::Capture { .. }))
            && self.sessions[s].schematic.choice_open()
            && self.sessions[s].player.health() > 0;
        let result = if !(self.may_edit(s) || authoring) {
            Err("Creative tools require creative mode".into())
        } else {
            match request {
                Pending::Action(action) => self.apply_creative(s, action, events),
                Pending::Placement { origin, turns, .. } => {
                    let design = decoded.expect("a placement is decoded before it is its turn");
                    self.place_schematic(s, design.schematic(), origin, turns, events)
                }
            }
        };
        if let Err(message) = result {
            self.sessions[s].creative.refuse(message);
        }
    }

    fn apply_creative(
        &mut self,
        s: usize,
        action: CreativeAction,
        events: &mut TickEvents,
    ) -> Result<(), String> {
        match action {
            CreativeAction::Capture {
                name,
                regions,
                include_air,
            } => self.begin_capture(s, name, regions, include_air),
            CreativeAction::Place { .. } => Err("A paste is queued through its own request".into()),
            CreativeAction::Undo => self.replay_edit(s, Replay::Undo, events),
            CreativeAction::Redo => self.replay_edit(s, Replay::Redo, events),
        }
    }
}

#[cfg(test)]
mod tests;
