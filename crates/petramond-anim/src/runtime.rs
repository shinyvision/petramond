use std::sync::Arc;

use super::graph::{
    ClipRef, EventId, ExprId, Graph, ParamId, Pick, PlayRule, SlotId, BUILTIN_DT, BUILTIN_TIME,
    SLOT_VARS,
};
use super::library::ClipId;
use super::pose::LocalPose;

mod eval;
mod slot;

use slot::{Montage, Rate, Segment, SlotRt};
pub use slot::{PlayId, PlaySpec, PlayState, Playing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiredMarker {
    pub clip: ClipId,
    pub marker: usize,
    pub mirrored: bool,
}

const MARKER_WEIGHT: f32 = 0.5;

const MAX_STEP: f32 = 0.25;

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

#[derive(Clone, Copy)]
enum Ground<'a> {
    Rest,
    Over(&'a LocalPose),
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
    gates_open: Vec<bool>,
    pending: Vec<EventId>,
    rng: u32,
    time: f64,
    dt: f32,
    frame: u64,
    last_play: u64,
    mirrored: bool,
    pose: LocalPose,
    pool: Vec<LocalPose>,
    markers: Vec<FiredMarker>,
    sought: Vec<FiredMarker>,
}

impl Animator {
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

    pub fn reset(&mut self) {
        let g = &*self.graph;
        self.vars = vec![0.0; g.vars.len];
        for (i, (_, default)) in g.params.iter().enumerate() {
            self.vars[i] = *default;
        }
        self.expr_state = vec![0.0; g.expr_state];
        self.clocks = vec![0.0; g.clip_nodes];
        self.phases = vec![0.0; g.blend_nodes];
        self.machines = (0..g.machines)
            .map(|_| eval::MachineRt::default())
            .collect();
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

    pub fn set_named(&mut self, name: &str, value: f32) -> bool {
        let Some(param) = self.graph.param(name) else {
            return false;
        };
        self.set(param, value);
        true
    }

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

    pub fn play(&mut self, slot: SlotId, spec: &PlaySpec) -> Option<PlayId> {
        if slot.index() >= self.slots.len() || spec.clip.index() >= self.graph.clips.len() {
            return None;
        }
        let id = self.next_play_id();
        self.slots[slot.index()]
            .start(Montage::from_spec(spec, id))
            .then_some(id)
    }

    pub fn stop(&mut self, slot: SlotId, fade: f32) {
        if let Some(slot) = self.slots.get_mut(slot.index()) {
            slot.stop(fade);
        }
    }

    pub fn stop_play(&mut self, slot: SlotId, play: PlayId, fade: f32) {
        if let Some(slot) = self.slots.get_mut(slot.index()) {
            slot.stop_play(play, fade);
        }
    }

    pub fn freeze(&mut self, slot: SlotId, seconds: f32) {
        if let Some(slot) = self.slots.get_mut(slot.index()) {
            slot.freeze = slot.freeze.max(seconds);
        }
    }

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

    pub fn play_state(&self, slot: SlotId, play: PlayId) -> PlayState {
        self.slots
            .get(slot.index())
            .map_or(PlayState::Displaced, |rt| rt.state(play))
    }

    pub fn playing(&self, slot: SlotId) -> Option<Playing> {
        self.slots.get(slot.index())?.playing(&self.graph)
    }

    pub fn pose(&self) -> &LocalPose {
        &self.pose
    }

    pub fn markers(&self) -> &[FiredMarker] {
        &self.markers
    }

    pub fn update(&mut self, dt: f32) {
        self.step(dt, Ground::Rest);
    }

    pub fn update_over(&mut self, dt: f32, ground: &LocalPose) {
        self.step(dt, Ground::Over(ground));
    }

    pub fn advance(&mut self, dt: f32) {
        self.step(dt, Ground::Unposed);
    }

    fn step(&mut self, dt: f32, ground: Ground<'_>) {
        let graph = Arc::clone(&self.graph);
        let g = &*graph;
        self.dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_STEP)
        } else {
            0.0
        };
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

    fn publish_slot_vars(&mut self, g: &Graph) {
        for (i, slot) in self.slots.iter().enumerate() {
            let base = g.vars.slots + i * SLOT_VARS;
            self.vars[base..base + SLOT_VARS].copy_from_slice(&slot.vars(g));
        }
    }

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
            let played = play
                .is_none_or(|(play, pick, segments)| self.play_rule(g, i, play, pick, segments));
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
