//! Montage slots: one-shot clips (and their follow-up segments) that events
//! and code start over whatever the layers beneath are doing.

use super::Animator;
use crate::animation::graph::{Ease, ExprId, Graph, Until};
use crate::animation::inertia::{settle_halflife, Inertia};
use crate::animation::library::ClipId;

/// A montage started from code rather than a graph rule.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySpec {
    pub clip: ClipId,
    pub rate: f32,
    /// Play the clip over exactly this many seconds instead of at `rate`.
    pub duration: Option<f32>,
    pub fade_in: f32,
    pub fade_out: f32,
    pub ease: Ease,
    /// Cut in and out at full weight and let inertialization absorb the
    /// jump (`fade_in` / `fade_out` are then settle times).
    pub inertial: bool,
    pub mirror: bool,
    /// Loop until stopped.
    pub looping: bool,
    /// A playing montage refuses a newcomer of lower priority.
    pub priority: i32,
}

impl PlaySpec {
    pub fn new(clip: ClipId) -> Self {
        Self {
            clip,
            rate: 1.0,
            duration: None,
            fade_in: 0.1,
            fade_out: 0.15,
            ease: Ease::default(),
            inertial: false,
            mirror: false,
            looping: false,
            priority: 0,
        }
    }
}

/// The handle [`Animator::play`] answers: it addresses that one montage —
/// to seek it, stop it, or ask where it stands — whatever else starts in
/// the slot afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayId(pub(super) u64);

/// Where a montage a handle names stands ([`Animator::play_state`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    /// In its slot and playing.
    Playing,
    /// It reached its end on its own.
    Finished,
    /// Something else took the slot from it: a newer montage cut in or
    /// faded in over it, or the slot was stopped.
    Displaced,
}

/// What a slot is playing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub clip: ClipId,
    /// Seconds into the current clip.
    pub time: f32,
    /// 0..1 through the current clip.
    pub progress: f32,
    pub weight: f32,
}

#[derive(Clone, Copy)]
pub(super) enum Rate {
    Const(f32),
    Expr(ExprId),
}

#[derive(Clone, Copy)]
pub(super) struct Segment {
    pub clip: ClipId,
    pub until: Until,
    pub rate: Option<Rate>,
}

pub(super) struct Montage {
    pub id: PlayId,
    pub segments: Vec<Segment>,
    pub rate: Rate,
    pub duration: Option<Rate>,
    pub fade_in: f32,
    pub fade_out: f32,
    pub ease: Ease,
    pub inertial: bool,
    pub mirror: bool,
    pub priority: i32,
    seg: usize,
    time: f32,
    age: f32,
    /// (elapsed, duration) once it is leaving.
    out: Option<(f32, f32)>,
    /// The frame an ended montage holds while it fades.
    hold_at: Option<f32>,
    /// A newer montage in the slot is fading in over this one.
    interrupted: bool,
}

impl Montage {
    pub fn new(segments: Vec<Segment>, rate: Rate, id: PlayId) -> Self {
        Self {
            id,
            segments,
            rate,
            duration: None,
            fade_in: 0.1,
            fade_out: 0.15,
            ease: Ease::default(),
            inertial: false,
            mirror: false,
            priority: 0,
            seg: 0,
            time: 0.0,
            age: 0.0,
            out: None,
            hold_at: None,
            interrupted: false,
        }
    }

    pub fn from_spec(spec: &PlaySpec, id: PlayId) -> Self {
        let until = if spec.looping { Until::Forever } else { Until::Once };
        let segment = Segment { clip: spec.clip, until, rate: None };
        let mut montage = Montage::new(vec![segment], Rate::Const(spec.rate), id);
        montage.duration = spec.duration.map(Rate::Const);
        montage.fade_in = spec.fade_in.max(0.0);
        montage.fade_out = spec.fade_out.max(0.0);
        montage.ease = spec.ease;
        montage.inertial = spec.inertial;
        montage.mirror = spec.mirror;
        montage.priority = spec.priority;
        montage
    }

    pub fn weight(&self) -> f32 {
        let fade_in = if self.inertial || self.fade_in <= 0.0 {
            1.0
        } else {
            self.ease.apply(self.age / self.fade_in)
        };
        let fade_out = match self.out {
            None => 1.0,
            Some((_, duration)) if duration <= 0.0 => 0.0,
            Some((elapsed, duration)) => 1.0 - self.ease.apply(elapsed / duration),
        };
        fade_in * fade_out
    }

    pub fn segment(&self) -> Segment {
        self.segments[self.seg]
    }

    /// Seconds into the current clip, as sampled.
    pub fn local_time(&self, len: f32) -> f32 {
        if let Some(t) = self.hold_at {
            return t;
        }
        match self.segment().until {
            Until::Once => self.time.clamp(0.0, len.max(0.0)),
            _ if len > 0.0 => self.time.rem_euclid(len),
            _ => 0.0,
        }
    }

    fn position(&self, g: &Graph) -> (ClipId, f32, f32) {
        let clip = self.segment().clip;
        let len = g.clips.get(clip).length;
        let t = self.local_time(len);
        (clip, t, if len > 0.0 { t / len } else { 1.0 })
    }

    /// Scrub to `seconds` into the current clip (clamped to it). Answers
    /// the forward stretch crossed, for its markers: `(clip, from, to,
    /// mirrored)`, or `None` when it moved backward or was already leaving.
    pub fn seek(&mut self, g: &Graph, seconds: f32) -> Option<(ClipId, f32, f32, bool)> {
        let clip = self.segment().clip;
        let len = g.clips.get(clip).length.max(0.0);
        // A looping segment wraps AT its length; stop a hair short so the
        // last frame of a scrub is the clip's end, not its start.
        let end = match self.segment().until {
            Until::Once => len,
            _ => (len - 1e-4).max(0.0),
        };
        let to = seconds.clamp(0.0, end);
        let from = self.time;
        self.time = to;
        self.hold_at = None;
        (to > from && self.out.is_none() && !self.interrupted).then_some((clip, from, to, self.mirror))
    }

    /// Begin leaving at its end; answers the settle time inertialization
    /// must absorb.
    fn end(&mut self) -> Option<f32> {
        if self.out.is_some() {
            return None;
        }
        if self.inertial {
            self.out = Some((0.0, 0.0));
            Some(self.fade_out)
        } else {
            self.out = Some((0.0, self.fade_out));
            None
        }
    }

    /// Begin leaving early over `fade` seconds (an inertial montage cuts at
    /// once); answers whether the cut needs inertialization.
    fn leave(&mut self, fade: f32) -> bool {
        if self.out.is_some() {
            return false;
        }
        self.out = Some((0.0, if self.inertial { 0.0 } else { fade.max(0.0) }));
        self.inertial
    }
}

#[derive(Default)]
pub(super) struct SlotRt {
    pub montages: Vec<Montage>,
    pub freeze: f32,
    pub inertia: Inertia,
    /// The last [`FINISHED_KEPT`] montages that reached their end and left —
    /// how a handle to one that is gone still reads finished.
    finished: Vec<PlayId>,
}

/// How many finished handles a slot remembers: enough that a caller asking
/// a few updates late, with other montages finishing meanwhile, still hears
/// that its own play ended rather than that it was displaced.
const FINISHED_KEPT: usize = 8;

fn remember_finished(finished: &mut Vec<PlayId>, play: PlayId) {
    if finished.len() == FINISHED_KEPT {
        finished.remove(0);
    }
    finished.push(play);
}

impl SlotRt {
    /// Would a montage of `priority` be admitted over what is playing?
    pub fn accepts(&self, priority: i32) -> bool {
        !self
            .montages
            .last()
            .is_some_and(|top| top.out.is_none() && top.priority > priority)
    }

    pub fn start(&mut self, montage: Montage) -> bool {
        if !self.accepts(montage.priority) {
            return false;
        }
        if montage.inertial {
            self.montages.clear();
            self.inertia.trigger(settle_halflife(montage.fade_in));
        } else {
            for old in &mut self.montages {
                old.interrupted = true;
            }
        }
        self.montages.push(montage);
        true
    }

    pub fn stop(&mut self, fade: f32) {
        let mut cut = false;
        for m in &mut self.montages {
            cut |= m.leave(fade);
        }
        if cut {
            self.inertia.trigger(settle_halflife(fade));
        }
    }

    pub fn stop_play(&mut self, play: PlayId, fade: f32) {
        let cut = self
            .montages
            .iter_mut()
            .find(|m| m.id == play)
            .is_some_and(|m| m.leave(fade));
        if cut {
            self.inertia.trigger(settle_halflife(fade));
        }
    }

    pub fn state(&self, play: PlayId) -> PlayState {
        match self.montages.iter().find(|m| m.id == play) {
            Some(m) if m.hold_at.is_some() => PlayState::Finished,
            Some(m) if m.out.is_none() && !m.interrupted => PlayState::Playing,
            Some(_) => PlayState::Displaced,
            None if self.finished.contains(&play) => PlayState::Finished,
            None => PlayState::Displaced,
        }
    }

    /// `slot.<name>`, `.time`, `.progress`, `.clip`.
    pub fn vars(&self, g: &Graph) -> [f32; 4] {
        let Some(top) = self.montages.last() else {
            return [0.0; 4];
        };
        let coverage = 1.0 - self.montages.iter().fold(1.0, |rest, m| rest * (1.0 - m.weight()));
        let (clip, time, progress) = top.position(g);
        [coverage, time, progress, g.clips.interned(clip)]
    }

    pub fn playing(&self, g: &Graph) -> Option<Playing> {
        let top = self.montages.last()?;
        let (clip, time, progress) = top.position(g);
        Some(Playing { clip, time, progress, weight: top.weight() })
    }

    fn settle_fades(&mut self) {
        // An interrupted montage leaves with whatever interrupted it, so it
        // can never resurface from under a newcomer's fade-out.
        for k in (0..self.montages.len().saturating_sub(1)).rev() {
            if self.montages[k].interrupted && self.montages[k].out.is_none() {
                if let Some(out) = self.montages[k + 1].out {
                    self.montages[k].out = Some(out);
                }
            }
        }
        let finished = &mut self.finished;
        self.montages.retain(|m| {
            let gone = m.out.is_some_and(|(elapsed, duration)| elapsed >= duration);
            if gone && m.hold_at.is_some() {
                remember_finished(finished, m.id);
            }
            !gone
        });
        if let Some(k) = self.montages.iter().rposition(|m| m.weight() >= 1.0) {
            for m in self.montages.drain(..k) {
                if m.hold_at.is_some() {
                    remember_finished(&mut self.finished, m.id);
                }
            }
        }
    }
}

impl Animator {
    pub(super) fn advance_slot(&mut self, g: &Graph, index: usize) {
        let mut slot = std::mem::take(&mut self.slots[index]);
        let frozen = self.dt.min(slot.freeze.max(0.0));
        let clip_dt = self.dt - frozen;
        slot.freeze -= frozen;
        let mut settle = None;
        for m in &mut slot.montages {
            m.age += self.dt;
            if let Some((elapsed, _)) = &mut m.out {
                *elapsed += self.dt;
            }
            settle = self.advance_montage(g, m, clip_dt).or(settle);
        }
        if let Some(settle) = settle {
            slot.inertia.trigger(settle_halflife(settle));
        }
        slot.settle_fades();
        self.slots[index] = slot;
    }

    fn advance_montage(&mut self, g: &Graph, m: &mut Montage, dt: f32) -> Option<f32> {
        if m.hold_at.is_some() {
            return None;
        }
        let seg = m.segment();
        let len = g.clips.get(seg.clip).length.max(0.0);
        let rate = self.montage_rate(g, m, seg, len);
        let prev = m.time;
        m.time += dt * rate;
        // A montage's markers are its action's, so they fire from its first
        // frame whatever its fade — but never after it was cut short.
        let fire = !m.interrupted && m.out.is_none();
        let last = m.seg + 1 == m.segments.len();
        let mut settle = None;
        match seg.until {
            Until::Once => {
                if fire {
                    self.cross(g, seg.clip, prev, m.time, false, m.mirror);
                }
                if m.time >= len {
                    if last {
                        m.hold_at = Some(len);
                        if !m.interrupted {
                            settle = m.end();
                        }
                    } else {
                        m.time -= len;
                        m.seg += 1;
                        if fire {
                            self.cross(g, m.segment().clip, 0.0, m.time, false, m.mirror);
                        }
                    }
                }
            }
            Until::Forever => {
                if fire {
                    self.cross(g, seg.clip, prev, m.time, true, m.mirror);
                }
                wrap(m, len);
            }
            Until::While { cond, finish_cycle } => {
                if fire {
                    self.cross(g, seg.clip, prev, m.time, true, m.mirror);
                }
                let holding = m.out.is_none() && self.expr(g, cond) != 0.0;
                let crossed = len <= 0.0 || m.time >= len;
                if holding || (finish_cycle && !crossed) {
                    wrap(m, len);
                } else if last {
                    m.hold_at = Some(if finish_cycle { len } else { m.time.rem_euclid(len.max(1e-6)) });
                    if !m.interrupted {
                        settle = m.end();
                    }
                } else {
                    m.time = if finish_cycle && len > 0.0 { (m.time - len).min(len) } else { 0.0 };
                    m.seg += 1;
                    if fire {
                        self.cross(g, m.segment().clip, 0.0, m.time, false, m.mirror);
                    }
                    if !finish_cycle {
                        settle = Some(m.fade_out);
                    }
                }
            }
        }
        // A cross-faded montage starts leaving so its fade completes as its
        // final clip does. An interrupted one holds and leaves with whatever
        // covers it.
        if last && !m.inertial && !m.interrupted && m.out.is_none() && seg.until == Until::Once && rate > 0.0 {
            let remaining = ((len - m.time) / rate).max(0.0);
            if remaining < m.fade_out {
                m.out = Some((m.fade_out - remaining, m.fade_out));
            }
        }
        settle
    }

    fn montage_rate(&mut self, g: &Graph, m: &Montage, seg: Segment, len: f32) -> f32 {
        let base = match m.duration {
            Some(duration) => {
                let duration = self.rate(g, duration);
                let once: f32 = m
                    .segments
                    .iter()
                    .filter(|s| s.until == Until::Once)
                    .map(|s| g.clips.get(s.clip).length)
                    .sum();
                let total = if once > 0.0 { once } else { len };
                if duration > 0.0 {
                    total / duration
                } else {
                    1.0
                }
            }
            None => self.rate(g, m.rate),
        };
        let scale = match seg.rate {
            Some(rate) => self.rate(g, rate),
            None => 1.0,
        };
        (base * scale).max(0.0)
    }
}

fn wrap(m: &mut Montage, len: f32) {
    if len > 0.0 && m.time >= len {
        m.time = m.time.rem_euclid(len);
    }
}
