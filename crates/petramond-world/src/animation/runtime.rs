//! One animated character's animator: the mutable half of a [`Graph`]. A
//! driver sets params, fires events and plays montages, then calls
//! [`update`](Animator::update) once per frame and reads the pose and the
//! markers its clips crossed — or [`advance`](Animator::advance) on a frame
//! nobody draws it, which keeps its rules and montages live without posing.
//! A marker the graph maps to an event fires that event on the next update.

use std::sync::Arc;

use super::graph::{
    ClipRef, EventId, ExprId, Graph, ParamId, Pick, PlayRule, SlotId, BUILTIN_DT, BUILTIN_TIME,
    SLOT_VARS,
};
use super::library::ClipId;
use super::pose::LocalPose;

mod eval;
mod slot;

pub use slot::{PlayId, PlaySpec, PlayState, Playing};
use slot::{Montage, Rate, Segment, SlotRt};

/// A marker a clip crossed during the last update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiredMarker {
    pub clip: ClipId,
    /// Index into the clip's `markers()`.
    pub marker: usize,
    /// Crossed while mirrored: a marker authored on one side belongs to the other.
    pub mirrored: bool,
}

/// The share of the final pose a clip needs for its markers to fire.
const MARKER_WEIGHT: f32 = 0.5;

/// The longest step one update integrates; a longer hitch plays as this.
const MAX_STEP: f32 = 0.25;

/// `anim.time` wraps here: the clock is f64, but an f32 input keeps sub-millisecond
/// phase only within an hour.
const TIME_WRAP: f64 = 3600.0;

#[derive(Clone, Copy)]
struct RuleRt {
    last: f64,
    next: usize,
    last_pick: Option<usize>,
}

impl Default for RuleRt {
    fn default() -> Self {
        Self {
            last: f64::NEG_INFINITY,
            next: 0,
            last_pick: None,
        }
    }
}

/// What an update evaluates the node tree over.
#[derive(Clone, Copy)]
enum Ground<'a> {
    Rest,
    Over(&'a LocalPose),
    /// No evaluation at all: [`Animator::advance`].
    Unposed,
}

pub struct Animator {
    graph: Arc<Graph>,
    vars: Vec<f32>,
    expr_state: Vec<f32>,
    clocks: Vec<f32>,
    phases: Vec<f32>,
    machines: Vec<eval::MachineRt>,
    slots: Vec<SlotRt>,
    rules: Vec<RuleRt>,
    /// Each gate's formula as it read when this update's events arrived.
    gates_open: Vec<bool>,
    pending: Vec<EventId>,
    rng: u32,
    time: f64,
    dt: f32,
    frame: u64,
    /// The last [`PlayId`] handed out; never reset, so a handle from before
    /// a [`reset`](Animator::reset) can never name a later montage.
    last_play: u64,
    mirrored: bool,
    pose: LocalPose,
    pool: Vec<LocalPose>,
    markers: Vec<FiredMarker>,
    /// Markers a [`seek`](Animator::seek) passed, reported by the next update.
    sought: Vec<FiredMarker>,
}

impl Animator {
    /// A fresh animator; `seed` drives random clip picks.
    pub fn new(graph: Arc<Graph>, seed: u32) -> Self {
        let mut animator = Self {
            vars: Vec::new(),
            expr_state: Vec::new(),
            clocks: Vec::new(),
            phases: Vec::new(),
            machines: Vec::new(),
            slots: Vec::new(),
            rules: Vec::new(),
            gates_open: Vec::new(),
            pending: Vec::new(),
            rng: if seed == 0 { 0x9E37_79B9 } else { seed },
            time: 0.0,
            dt: 0.0,
            frame: 0,
            last_play: 0,
            mirrored: false,
            pose: LocalPose::rest(graph.bones),
            pool: Vec::new(),
            markers: Vec::new(),
            sought: Vec::new(),
            graph,
        };
        animator.reset();
        animator
    }

    /// Back to the graph's initial state: every machine in its initial
    /// state, every slot empty, params at their defaults.
    pub fn reset(&mut self) {
        let g = &*self.graph;
        self.vars = vec![0.0; g.vars.len];
        for (i, (_, default)) in g.params.iter().enumerate() {
            self.vars[i] = *default;
        }
        self.expr_state = vec![0.0; g.expr_state];
        self.clocks = vec![0.0; g.clip_nodes];
        self.phases = vec![0.0; g.blend_nodes];
        self.machines = (0..g.machines).map(|_| eval::MachineRt::default()).collect();
        self.slots = (0..g.slots.len()).map(|_| SlotRt::default()).collect();
        self.rules = vec![RuleRt::default(); g.rules.len()];
        self.gates_open = vec![true; g.gates.len()];
        self.pending.clear();
        self.markers.clear();
        self.sought.clear();
        self.pose = LocalPose::rest(g.bones);
    }

    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }

    pub fn set(&mut self, param: ParamId, value: f32) {
        if (param.0 as usize) < self.graph.params.len() {
            self.vars[param.0 as usize] = if value.is_finite() { value } else { 0.0 };
        }
    }

    pub fn param(&self, param: ParamId) -> f32 {
        self.vars.get(param.0 as usize).copied().unwrap_or(0.0)
    }

    /// Set a param by name; `false` when the graph declares no such param.
    pub fn set_named(&mut self, name: &str, value: f32) -> bool {
        let Some(param) = self.graph.param(name) else {
            return false;
        };
        self.set(param, value);
        true
    }

    /// Queue an event for the next update's rules and `event.*` inputs. An
    /// event is a level, not a count: firing it twice before an update is one
    /// firing.
    pub fn fire(&mut self, event: EventId) {
        if (event.0 as usize) < self.graph.events.len() && !self.pending.contains(&event) {
            self.pending.push(event);
        }
    }

    pub fn fire_named(&mut self, name: &str) -> bool {
        let Some(event) = self.graph.event(name) else {
            return false;
        };
        self.fire(event);
        true
    }

    /// Start a montage in `slot`, answering the handle that addresses it
    /// from then on; `None` when a higher-priority montage playing there
    /// refuses it.
    pub fn play(&mut self, slot: SlotId, spec: &PlaySpec) -> Option<PlayId> {
        if slot.index() >= self.slots.len() || spec.clip.index() >= self.graph.clips.len() {
            return None;
        }
        let id = self.next_play_id();
        self.slots[slot.index()]
            .start(Montage::from_spec(spec, id))
            .then_some(id)
    }

    /// Fade out everything playing in `slot` over `fade` seconds.
    pub fn stop(&mut self, slot: SlotId, fade: f32) {
        if let Some(slot) = self.slots.get_mut(slot.index()) {
            slot.stop(fade);
        }
    }

    /// Fade out the one montage `play` names over `fade` seconds; whatever
    /// else plays in the slot plays on.
    pub fn stop_play(&mut self, slot: SlotId, play: PlayId, fade: f32) {
        if let Some(slot) = self.slots.get_mut(slot.index()) {
            slot.stop_play(play, fade);
        }
    }

    /// Freeze `slot`'s clips (not its fades) for `seconds` — what a rule's
    /// `hitstop` key does. A longer freeze already running keeps its remainder.
    pub fn freeze(&mut self, slot: SlotId, seconds: f32) {
        if let Some(slot) = self.slots.get_mut(slot.index()) {
            slot.freeze = slot.freeze.max(seconds);
        }
    }

    /// Put the montage `play` names at `seconds` into its current clip — how
    /// a montage started at rate 0 is SCRUBBED by a clock the caller owns.
    /// Moving forward fires the markers passed on the way. A play no longer
    /// in its slot moves nothing, whatever took its place.
    pub fn seek(&mut self, slot: SlotId, play: PlayId, seconds: f32) {
        let graph = Arc::clone(&self.graph);
        let Some(mut rt) = self.slots.get_mut(slot.index()).map(std::mem::take) else {
            return;
        };
        if let Some(m) = rt.montages.iter_mut().find(|m| m.id == play) {
            if let Some((clip, from, to, mirror)) = m.seek(&graph, seconds) {
                let start = self.markers.len();
                self.cross(&graph, clip, from, to, false, mirror);
                let passed: Vec<FiredMarker> = self.markers.drain(start..).collect();
                self.sought.extend(passed);
            }
        }
        self.slots[slot.index()] = rt;
    }

    /// Where the montage `play` names stands.
    pub fn play_state(&self, slot: SlotId, play: PlayId) -> PlayState {
        self.slots
            .get(slot.index())
            .map_or(PlayState::Displaced, |rt| rt.state(play))
    }

    /// The newest montage in `slot`, if any.
    pub fn playing(&self, slot: SlotId) -> Option<Playing> {
        self.slots.get(slot.index())?.playing(&self.graph)
    }

    /// The pose the last posed update produced.
    pub fn pose(&self) -> &LocalPose {
        &self.pose
    }

    /// The markers crossed during the last update, each once.
    pub fn markers(&self) -> &[FiredMarker] {
        &self.markers
    }

    pub fn update(&mut self, dt: f32) {
        self.step(dt, Ground::Rest);
    }

    /// [`update`](Self::update) over `ground` instead of rest: the pose the
    /// root layer stack starts from — a locomotion pose the graph's actions
    /// override and add to. A ground for a different rig is ignored.
    pub fn update_over(&mut self, dt: f32, ground: &LocalPose) {
        self.step(dt, Ground::Over(ground));
    }

    /// One update's worth of time for a frame that draws nothing: params,
    /// events, gates, rules, montages and marker events run as in
    /// [`update`](Self::update), but no node is evaluated, so the pose, the
    /// layers' clip clocks, machines and the stateful formulas inside nodes
    /// hold until the next posed update.
    pub fn advance(&mut self, dt: f32) {
        self.step(dt, Ground::Unposed);
    }

    fn step(&mut self, dt: f32, ground: Ground<'_>) {
        let graph = Arc::clone(&self.graph);
        let g = &*graph;
        self.dt = if dt.is_finite() { dt.clamp(0.0, MAX_STEP) } else { 0.0 };
        self.time += f64::from(self.dt);
        self.frame += 1;
        self.markers.clear();
        self.markers.append(&mut self.sought);

        let builtins = g.vars.builtins;
        self.vars[builtins + BUILTIN_DT] = self.dt;
        self.vars[builtins + BUILTIN_TIME] = (self.time % TIME_WRAP) as f32;
        self.vars[g.vars.events..g.vars.slots].fill(0.0);
        self.publish_slot_vars(g);
        let mut pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            for event in &pending {
                self.vars[g.vars.events + event.0 as usize] = 1.0;
            }
            for (i, gate) in g.gates.iter().enumerate() {
                self.gates_open[i] = self.expr(g, gate.when) != 0.0;
            }
            for event in &pending {
                self.run_rules(g, *event);
            }
        }
        pending.clear();
        self.pending = pending;

        for slot in 0..self.slots.len() {
            self.advance_slot(g, slot);
        }
        self.publish_slot_vars(g);

        let over = match ground {
            Ground::Rest => Some(None),
            Ground::Over(pose) => Some(Some(pose)),
            Ground::Unposed => None,
        };
        if let Some(over) = over {
            let mut base = self.take_rest(g.bones);
            if let Some(ground) = over.filter(|p| p.len() == g.bones) {
                base.copy_from(ground);
            }
            let ground = base;
            let mut out = self.take(&ground);
            self.eval(g, g.root, &ground, &mut out, 1.0);
            std::mem::swap(&mut self.pose, &mut out);
            self.give(ground);
            self.give(out);
        }

        let mut i = 0;
        while i < self.markers.len() {
            if self.markers[..i].contains(&self.markers[i]) {
                self.markers.remove(i);
            } else {
                i += 1;
            }
        }
        self.fire_marker_events(g);
    }

    /// The `slot.*` inputs, from the slots as they stand now.
    fn publish_slot_vars(&mut self, g: &Graph) {
        for (i, slot) in self.slots.iter().enumerate() {
            let base = g.vars.slots + i * SLOT_VARS;
            self.vars[base..base + SLOT_VARS].copy_from_slice(&slot.vars(g));
        }
    }

    /// Every crossed marker the graph maps to an event fires it for the next
    /// update.
    fn fire_marker_events(&mut self, g: &Graph) {
        for i in 0..self.markers.len() {
            let crossed = self.markers[i];
            if let Some(event) = g.marker_event(crossed.clip, crossed.marker, crossed.mirrored) {
                self.fire(event);
            }
        }
    }

    fn run_rules(&mut self, g: &Graph, event: EventId) {
        for (i, rule) in g.rules.iter().enumerate() {
            if rule.event != event {
                continue;
            }
            let since = self.time - self.rules[i].last;
            if since < f64::from(rule.cooldown)
                || !rule.gates.iter().all(|&gate| self.gates_open[gate])
                || self.expr(g, rule.when) == 0.0
            {
                continue;
            }
            // A play the slot refuses, or whose clip template names nothing
            // for its param, is no match: the rule keeps its cooldown and its
            // combo pick, and the next rule gets the event.
            if let Some(play) = &rule.play {
                if !self.slots[play.slot.index()].accepts(play.priority) {
                    continue;
                }
            }
            let play = match &rule.play {
                Some(play) => {
                    let pick = self.pick(i, play.pick, play.first.len(), since);
                    let Some(segments) = self.resolve_segments(g, play, pick) else {
                        continue;
                    };
                    Some((play, pick, segments))
                }
                None => None,
            };
            if let Some((slot, fade)) = rule.stop {
                self.slots[slot.index()].stop(fade);
            }
            let played = play.is_none_or(|(play, pick, segments)| {
                self.play_rule(g, i, play, pick, segments)
            });
            if played {
                self.rules[i].last = self.time;
                if let Some((slot, seconds)) = rule.freeze {
                    self.freeze(slot, seconds);
                }
            }
            if !rule.fallthrough {
                break;
            }
        }
    }

    /// The segments `play` runs with alternative `pick` first, each clip
    /// template resolved by its param as it stands; `None` when a template
    /// names no clip for that value.
    fn resolve_segments(&self, g: &Graph, play: &PlayRule, pick: usize) -> Option<Vec<Segment>> {
        std::iter::once(&play.first[pick])
            .chain(&play.then)
            .map(|def| {
                Some(Segment {
                    clip: self.resolve_clip(g, def.clip)?,
                    until: def.until,
                    rate: def.rate.map(Rate::Expr),
                })
            })
            .collect()
    }

    fn resolve_clip(&self, g: &Graph, clip: ClipRef) -> Option<ClipId> {
        match clip {
            ClipRef::Fixed(id) => Some(id),
            ClipRef::Template(t) => {
                let template = &g.templates[t];
                template
                    .clips
                    .get(&self.vars[template.param].to_bits())
                    .copied()
            }
        }
    }

    /// Start `play`'s montage; `false` when its slot refused it, in which
    /// case the pick is not committed either.
    fn play_rule(
        &mut self,
        g: &Graph,
        rule: usize,
        play: &PlayRule,
        pick: usize,
        segments: Vec<Segment>,
    ) -> bool {
        let id = self.next_play_id();
        let mut montage = Montage::new(segments, Rate::Expr(play.rate), id);
        montage.duration = play.duration.map(Rate::Expr);
        montage.fade_in = play.fade_in;
        montage.fade_out = play.fade_out;
        montage.ease = play.ease;
        montage.inertial = play.inertial;
        montage.mirror = self.expr(g, play.mirror) != 0.0;
        montage.priority = play.priority;
        let started = self.slots[play.slot.index()].start(montage);
        if started {
            let rt = &mut self.rules[rule];
            rt.next = pick + 1;
            rt.last_pick = Some(pick);
        }
        started
    }

    /// Which of `choices` alternatives `rule` plays next, without committing
    /// to it.
    fn pick(&mut self, rule: usize, pick: Pick, choices: usize, since: f64) -> usize {
        if choices <= 1 {
            return 0;
        }
        match pick {
            Pick::Cycle { reset } if since > f64::from(reset) => 0,
            Pick::Cycle { .. } => self.rules[rule].next % choices,
            Pick::Random => {
                let r = self.random() as usize;
                match self.rules[rule].last_pick {
                    Some(last) => (last + 1 + r % (choices - 1)) % choices,
                    None => r % choices,
                }
            }
        }
    }

    fn random(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    fn next_play_id(&mut self) -> PlayId {
        self.last_play += 1;
        PlayId(self.last_play)
    }

    fn expr(&mut self, g: &Graph, id: ExprId) -> f32 {
        g.exprs[id.0 as usize].eval(&self.vars, &mut self.expr_state, self.dt)
    }

    fn rate(&mut self, g: &Graph, rate: Rate) -> f32 {
        match rate {
            Rate::Const(v) => v,
            Rate::Expr(e) => self.expr(g, e),
        }
    }

    fn take(&mut self, like: &LocalPose) -> LocalPose {
        let mut pose = self.pool.pop().unwrap_or_default();
        pose.copy_from(like);
        pose
    }

    fn take_rest(&mut self, bones: usize) -> LocalPose {
        match self.pool.pop() {
            Some(mut pose) if pose.len() == bones => {
                pose.clear();
                pose
            }
            _ => LocalPose::rest(bones),
        }
    }

    fn give(&mut self, pose: LocalPose) {
        self.pool.push(pose);
    }
}

#[cfg(test)]
mod tests;
