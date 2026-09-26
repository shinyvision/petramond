//! Applying a body's resolved animator claims (params set, slots played, and
//! the graph events fired — [`AnimatorInputs`]) to one rig's [`Animator`] —
//! the client-side end of the mod ABI's `set` / `play` / `fire` primitives,
//! shared by the first-person viewmodel and every body.
//!
//! A frame runs in two halves around the engine driver's own inputs:
//! [`release`](ClaimDriver::release) first puts every param claimed last
//! frame back to the graph default, then the engine writes its values, then
//! [`claim`](ClaimDriver::claim) writes this frame's claims over them — so a
//! claim overrides the engine's value while it stands and the engine's value
//! is back THE FRAME it releases, never the default for one frame.
//!
//! A claimed play is a standing level, held by the handle of the montage it
//! started: a scrub seeks THAT montage and nothing else; a rule that takes
//! the slot displaces it, and the claim starts again as soon as the slot
//! admits it; a run that plays to its end stays ended while its claim
//! stands; and a play whose key — slot, clip, mirror, clock kind — changes
//! or leaves the claims stops only its own montage. Everything cuts
//! inertially, so a claim starting, releasing or changing clip keeps its
//! motion instead of cross-dissolving.

use petramond::player::{AnimatorClock, AnimatorParam, AnimatorPlay, AnimatorValue, RigId};
use petramond_anim::{
    Animator, ClipId, EventId, Graph, ParamId, PlayId, PlaySpec, PlayState, SlotId,
};
use petramond_render::ArenaRange;

/// One claimed graph param with its value resolved to the number the
/// animator takes — a name claim interned ONCE, where the claim arrives
/// ([`NameCache`]), never per frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AnimatorParamRow {
    pub rig: RigId,
    pub param: u16,
    pub value: f32,
}

/// Interned name values, cached by string so a claim that repeats a name
/// never takes the global intern lock again.
#[derive(Default)]
pub struct NameCache {
    names: rustc_hash::FxHashMap<Box<str>, f32>,
}

impl NameCache {
    pub fn row(&mut self, p: &AnimatorParam) -> AnimatorParamRow {
        let value = match &p.value {
            AnimatorValue::Number(v) => *v,
            AnimatorValue::Name(n) => match self.names.get(n.as_str()) {
                Some(v) => *v,
                None => {
                    let v = petramond_anim::expr::intern(n);
                    self.names.insert(n.as_str().into(), v);
                    v
                }
            },
        };
        AnimatorParamRow {
            rig: p.rig,
            param: p.param,
            value,
        }
    }

    /// Resolve every param of `claims` onto the end of `out`.
    pub fn rows(&mut self, params: &[AnimatorParam], out: &mut Vec<AnimatorParamRow>) {
        out.extend(params.iter().map(|p| self.row(p)));
    }
}

/// One body's resolved animator claims and the graph events fired on it
/// this frame, borrowed from the frame's arenas
/// ([`BodyFrame`](super::BodyFrame) carries every body's rows back to back;
/// a body addresses its own by [`AnimatorRanges`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct AnimatorInputs<'a> {
    pub params: &'a [AnimatorParamRow],
    pub plays: &'a [AnimatorPlay],
    pub events: &'a [(RigId, u16)],
}

/// One body's slices of the frame's animator arenas.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimatorRanges {
    pub params: ArenaRange,
    pub plays: ArenaRange,
    pub events: ArenaRange,
}

impl AnimatorRanges {
    /// This body's rows of the frame's arenas.
    pub fn of<'a>(
        self,
        params: &'a [AnimatorParamRow],
        plays: &'a [AnimatorPlay],
        events: &'a [(RigId, u16)],
    ) -> AnimatorInputs<'a> {
        AnimatorInputs {
            params: self.params.of(params),
            plays: self.plays.of(plays),
            events: self.events.of(events),
        }
    }
}

/// Seconds a claim takes to settle in, out, or from one clip into the next.
const CLAIM_SETTLE: f32 = 0.12;

/// The part of a play's clock that identifies the play: a scrub's progress
/// changes every frame without restarting it, a run's rate or loop does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum HeldClock {
    Scrub,
    Run { rate: f32, looping: bool },
}

impl HeldClock {
    fn of(clock: AnimatorClock) -> Self {
        match clock {
            AnimatorClock::Scrub(_) => HeldClock::Scrub,
            AnimatorClock::Run { rate, looping } => HeldClock::Run { rate, looping },
        }
    }
}

/// What identifies a claimed play: a changed key is a new play.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    slot: SlotId,
    clip: ClipId,
    mirror: bool,
    clock: HeldClock,
}

/// One claimed play and the montage standing for it.
#[derive(Clone, Copy, Debug)]
struct Held {
    key: Key,
    /// The montage this claim started, while the slot still has it.
    play: Option<PlayId>,
    /// A run that played to its end: it stays ended while the claim stands.
    done: bool,
}

pub(crate) struct ClaimDriver {
    rig: RigId,
    /// How many params, slots and clips the graph declares: a claim past
    /// them (a peer's graph that differs) is dropped.
    params: usize,
    slots: usize,
    clips: usize,
    /// Params claimed last frame, to restore when a claim releases.
    claimed: Vec<ParamId>,
    /// The plays claimed last frame.
    plays: Vec<Held>,
    /// Scratch for this frame's plays, swapped with `plays`.
    next: Vec<Held>,
}

impl ClaimDriver {
    pub fn new(rig: RigId, graph: &Graph) -> Self {
        Self {
            rig,
            params: graph.param_names().count(),
            slots: graph.slot_names().len(),
            clips: graph.clips().len(),
            claimed: Vec::new(),
            plays: Vec::new(),
            next: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.claimed.clear();
        self.plays.clear();
    }

    /// Put every param claimed last frame back to its graph default. Runs
    /// BEFORE the engine writes its own inputs, so a released param shows
    /// the engine's value this same frame.
    pub fn release(&mut self, animator: &mut Animator) {
        let graph = animator.graph().clone();
        for param in self.claimed.drain(..) {
            animator.set(param, graph.param_default(param));
        }
    }

    /// Write this frame's claims and events for this driver's rig, after the
    /// engine's inputs and before the animator's update.
    pub fn claim(&mut self, animator: &mut Animator, inputs: AnimatorInputs<'_>) {
        for p in inputs.params.iter().filter(|p| p.rig == self.rig) {
            if usize::from(p.param) >= self.params {
                continue;
            }
            let id = ParamId::from_index(p.param);
            animator.set(id, p.value);
            self.claimed.push(id);
        }

        let graph = animator.graph().clone();
        self.next.clear();
        for p in inputs.plays.iter().filter(|p| p.rig == self.rig) {
            if usize::from(p.slot) >= self.slots || usize::from(p.clip) >= self.clips {
                continue;
            }
            let key = Key {
                slot: SlotId::from_index(p.slot),
                clip: ClipId::from_index(usize::from(p.clip)),
                mirror: p.mirror,
                clock: HeldClock::of(p.clock),
            };
            let mut held = self
                .plays
                .iter()
                .find(|h| h.key == key)
                .copied()
                .unwrap_or(Held {
                    key,
                    play: None,
                    done: false,
                });
            if !held.done {
                match held.play.map(|id| animator.play_state(key.slot, id)) {
                    Some(PlayState::Playing) => {}
                    Some(PlayState::Finished) => {
                        held.play = None;
                        held.done = true;
                    }
                    // Refused while a higher-priority montage holds the
                    // slot: `None`, and the claim asks again next frame.
                    Some(PlayState::Displaced) | None => {
                        held.play = animator.play(key.slot, &spec(&key, p.priority));
                    }
                }
            }
            if let (Some(id), AnimatorClock::Scrub(progress)) = (held.play, p.clock) {
                let length = graph.clips().get(key.clip).length;
                animator.seek(key.slot, id, progress * length);
            }
            self.next.push(held);
        }
        for old in &self.plays {
            if let Some(id) = old.play {
                if !self.next.iter().any(|h| h.key == old.key) {
                    animator.stop_play(old.key.slot, id, CLAIM_SETTLE);
                }
            }
        }
        std::mem::swap(&mut self.plays, &mut self.next);

        for (rig, event) in inputs.events {
            if *rig == self.rig {
                animator.fire(EventId::from_index(*event));
            }
        }
    }
}

/// The montage a claimed play starts.
fn spec(key: &Key, priority: i32) -> PlaySpec {
    let mut spec = PlaySpec::new(key.clip);
    // A scrub never ends on its own: it loops so a seek past the last frame
    // still finds a playing clip.
    (spec.rate, spec.looping) = match key.clock {
        HeldClock::Scrub => (0.0, true),
        HeldClock::Run { rate, looping } => (rate, looping),
    };
    spec.inertial = true;
    spec.fade_in = CLAIM_SETTLE;
    spec.fade_out = CLAIM_SETTLE;
    spec.mirror = key.mirror;
    spec.priority = priority;
    spec
}

#[cfg(test)]
mod tests;
