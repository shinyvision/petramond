//! Node evaluation: each node writes its pose over the ground it is handed,
//! advancing its own clocks as it goes.

use glam::Vec3;

use super::{Animator, FiredMarker, MARKER_WEIGHT};
use crate::animation::graph::{
    ClipTime, Ease, Graph, MachineNode, Node, NodeId, BUILTIN_STATE_PROGRESS, BUILTIN_STATE_TIME,
};
use crate::animation::inertia::{settle_halflife, Inertia};
use crate::animation::library::ClipId;
use crate::animation::pose::LocalPose;
use crate::bbmodel::Channel;

#[derive(Default)]
pub(super) struct MachineRt {
    started: bool,
    current: usize,
    /// Oldest first; each entry fades in over everything beneath it.
    stack: Vec<Fade>,
    inertia: Inertia,
}

#[derive(Clone, Copy)]
struct Fade {
    state: usize,
    age: f32,
    duration: f32,
    ease: Ease,
}

impl Fade {
    fn alpha(&self) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            self.ease.apply(self.age / self.duration)
        }
    }
}

impl Animator {
    /// Write node `id`'s pose into `out`, which holds `ground` on entry.
    /// `weight` is the node's share of the final pose, which gates markers.
    pub(super) fn eval(
        &mut self,
        g: &Graph,
        id: NodeId,
        ground: &LocalPose,
        out: &mut LocalPose,
        weight: f32,
    ) {
        match &g.nodes[id.0 as usize] {
            Node::Pass => {}
            Node::Clip(c) => {
                let anim = g.clips.get(c.clip);
                let len = anim.length;
                let prev = self.clocks[c.clock];
                let mut t = match c.time {
                    ClipTime::Rate(rate) => prev + self.dt * self.expr(g, rate),
                    ClipTime::Seconds(time) => self.expr(g, time),
                    ClipTime::Progress(progress) => self.expr(g, progress) * len,
                };
                if weight >= MARKER_WEIGHT {
                    self.cross(g, c.clip, prev, t, c.looping, self.mirrored);
                }
                if c.looping && len > 0.0 && t >= len * 64.0 {
                    t = t.rem_euclid(len);
                }
                self.clocks[c.clock] = t;
                out.set_clip_at_looping(anim, sample_time(t, len, c.looping), c.looping);
            }
            Node::Blend(b) => {
                let x = self.expr(g, b.by);
                let (lo, hi, f) = locate(&b.points, x);
                let Some(rate) = b.sync else {
                    if f <= 0.0 || lo == hi {
                        self.eval(g, lo, ground, out, weight);
                    } else {
                        self.eval(g, lo, ground, out, weight * (1.0 - f));
                        let mut over = self.take(ground);
                        self.eval(g, hi, ground, &mut over, weight * f);
                        out.blend_toward(&over, f, None);
                        self.give(over);
                    }
                    return;
                };
                // One phase for every gait: each clip sits at the same fraction
                // of its own length, advancing at the weighted mean length.
                let rate = self.expr(g, rate);
                let clip_len = |node: NodeId| match &g.nodes[node.0 as usize] {
                    Node::Clip(c) => g.clips.get(c.clip).length,
                    _ => 0.0,
                };
                let span = clip_len(lo) * (1.0 - f) + clip_len(hi) * f;
                let from = self.phases[b.phase];
                let to = if span > 0.0 {
                    from + self.dt * rate / span
                } else {
                    from
                };
                let fire = weight >= MARKER_WEIGHT;
                let heaviest = if f > 0.5 { hi } else { lo };
                self.sync_clip(g, lo, from, to, out, fire && heaviest == lo);
                if f > 0.0 && lo != hi {
                    let mut over = self.take(ground);
                    self.sync_clip(g, hi, from, to, &mut over, fire && heaviest == hi);
                    out.blend_toward(&over, f, None);
                    self.give(over);
                }
                self.phases[b.phase] = to.rem_euclid(1.0);
            }
            Node::Machine(m) => self.eval_machine(g, m, ground, out, weight),
            Node::Layers(layers) => {
                for layer in layers {
                    let w = self.expr(g, layer.weight);
                    let w = if layer.additive { w } else { w.clamp(0.0, 1.0) };
                    if w == 0.0 {
                        continue;
                    }
                    let mask = layer.mask.map(|i| &*g.masks[i]);
                    if layer.additive {
                        let zero = self.take_rest(g.bones);
                        let mut delta = self.take_rest(g.bones);
                        self.eval(g, layer.node, &zero, &mut delta, weight * w.abs().min(1.0));
                        out.add_scaled(&delta, w, mask);
                        self.give(zero);
                        self.give(delta);
                    } else {
                        let base = self.take(out);
                        let mut over = self.take(out);
                        self.eval(g, layer.node, &base, &mut over, weight * w);
                        out.blend_toward(&over, w, mask);
                        self.give(base);
                        self.give(over);
                    }
                }
            }
            Node::Slot(slot) => {
                let mut rt = std::mem::take(&mut self.slots[slot.index()]);
                for m in &rt.montages {
                    let w = m.weight();
                    if w <= 0.0 {
                        continue;
                    }
                    let anim = g.clips.get(m.segment().clip);
                    let t = m.local_time(anim.length);
                    let mut over = self.take(ground);
                    if m.mirror {
                        let mut mirrored = self.take_rest(g.bones);
                        ground.mirror_into(&g.mirror, &mut mirrored);
                        mirrored.set_clip_at(anim, t);
                        mirrored.mirror_into(&g.mirror, &mut over);
                        self.give(mirrored);
                    } else {
                        over.set_clip_at(anim, t);
                    }
                    out.blend_toward(&over, w, None);
                    self.give(over);
                }
                rt.inertia.apply(out, self.dt, self.frame);
                self.slots[slot.index()] = rt;
            }
            Node::Bones(drives) => {
                for d in drives {
                    let v = Vec3::new(
                        self.expr(g, d.value[0]),
                        self.expr(g, d.value[1]),
                        self.expr(g, d.value[2]),
                    );
                    match d.channel {
                        Channel::Rotation => out.set_rotation(d.bone, v),
                        Channel::Position => out.set_position(d.bone, v),
                    }
                }
            }
            Node::Mirror { child, when } => {
                if self.expr(g, *when) == 0.0 {
                    self.eval(g, *child, ground, out, weight);
                    return;
                }
                let mut mirrored = self.take_rest(g.bones);
                ground.mirror_into(&g.mirror, &mut mirrored);
                let mut over = self.take(&mirrored);
                self.mirrored = !self.mirrored;
                self.eval(g, *child, &mirrored, &mut over, weight);
                self.mirrored = !self.mirrored;
                over.mirror_into(&g.mirror, out);
                self.give(mirrored);
                self.give(over);
            }
        }
    }

    fn eval_machine(
        &mut self,
        g: &Graph,
        m: &MachineNode,
        ground: &LocalPose,
        out: &mut LocalPose,
        weight: f32,
    ) {
        let mut rt = std::mem::take(&mut self.machines[m.index]);
        if !rt.started {
            rt.started = true;
            rt.current = m.initial;
            rt.stack.clear();
            rt.stack.push(Fade {
                state: m.initial,
                age: 0.0,
                duration: 0.0,
                ease: Ease::Linear,
            });
            self.reset_node(g, m.states[m.initial].1);
        }

        let age = rt.stack.last().map_or(0.0, |f| f.age);
        let progress = self.progress(g, m.states[rt.current].1);
        let saved = self.set_state_vars(g, age, progress);
        for t in &m.transitions {
            let applies = match t.from {
                Some(from) => from == rt.current,
                None => t.to != rt.current,
            };
            if !applies || self.expr(g, t.when) == 0.0 {
                continue;
            }
            let live = rt.stack.iter().any(|f| f.state == t.to);
            if !live {
                self.reset_node(g, m.states[t.to].1);
            }
            if t.inertial || t.fade <= 0.0 || live {
                // A state still fading out cannot fade in a second time without
                // running its clocks twice, so re-entry collapses the stack and
                // inertialization carries the difference instead.
                rt.stack.clear();
                rt.stack.push(Fade {
                    state: t.to,
                    age: 0.0,
                    duration: 0.0,
                    ease: t.ease,
                });
                if t.fade > 0.0 && (t.inertial || live) {
                    rt.inertia.trigger(settle_halflife(t.fade));
                }
            } else {
                rt.stack.push(Fade {
                    state: t.to,
                    age: 0.0,
                    duration: t.fade,
                    ease: t.ease,
                });
            }
            rt.current = t.to;
            break;
        }

        for fade in &mut rt.stack {
            fade.age += self.dt;
        }
        if let Some(k) = rt.stack.iter().rposition(|f| f.alpha() >= 1.0) {
            rt.stack.drain(..k);
        }
        for k in 0..rt.stack.len() {
            let fade = rt.stack[k];
            let alpha = if k == 0 { 1.0 } else { fade.alpha() };
            let cover: f32 = rt.stack[k + 1..].iter().map(|f| 1.0 - f.alpha()).product();
            let node = m.states[fade.state].1;
            let progress = self.progress(g, node);
            self.set_state_vars(g, fade.age, progress);
            if k == 0 {
                self.eval(g, node, ground, out, weight * cover);
            } else {
                let mut over = self.take(ground);
                self.eval(g, node, ground, &mut over, weight * alpha * cover);
                out.blend_toward(&over, alpha, None);
                self.give(over);
            }
        }
        self.set_state_vars(g, saved.0, saved.1);
        rt.inertia.apply(out, self.dt, self.frame);
        self.machines[m.index] = rt;
    }

    fn sync_clip(
        &mut self,
        g: &Graph,
        node: NodeId,
        from: f32,
        to: f32,
        out: &mut LocalPose,
        fire: bool,
    ) {
        let Node::Clip(c) = &g.nodes[node.0 as usize] else {
            return;
        };
        let anim = g.clips.get(c.clip);
        let len = anim.length;
        if fire {
            self.cross(g, c.clip, from * len, to * len, true, self.mirrored);
        }
        self.clocks[c.clock] = to.rem_euclid(1.0) * len;
        out.set_clip_at_looping(anim, sample_time(to * len, len, true), true);
    }

    /// Back to the start: clocks to zero, machines to their initial state.
    fn reset_node(&mut self, g: &Graph, node: NodeId) {
        match &g.nodes[node.0 as usize] {
            Node::Clip(c) => self.clocks[c.clock] = 0.0,
            Node::Blend(b) => {
                self.phases[b.phase] = 0.0;
                for (_, child) in &b.points {
                    self.reset_node(g, *child);
                }
            }
            Node::Machine(m) => self.machines[m.index].started = false,
            Node::Layers(layers) => {
                for layer in layers {
                    self.reset_node(g, layer.node);
                }
            }
            Node::Mirror { child, .. } => self.reset_node(g, *child),
            Node::Pass | Node::Slot(_) | Node::Bones(_) => {}
        }
    }

    /// How far through a node is, for `state.progress`: a clip's played
    /// length in clip lengths (past 1 once a one-shot has finished).
    fn progress(&self, g: &Graph, node: NodeId) -> f32 {
        match &g.nodes[node.0 as usize] {
            Node::Clip(c) => {
                let len = g.clips.get(c.clip).length;
                if len > 0.0 {
                    self.clocks[c.clock] / len
                } else {
                    1.0
                }
            }
            Node::Blend(b) if b.sync.is_some() => self.phases[b.phase],
            Node::Machine(m) => {
                let rt = &self.machines[m.index];
                if rt.started {
                    self.progress(g, m.states[rt.current].1)
                } else {
                    0.0
                }
            }
            Node::Layers(layers) => layers.first().map_or(0.0, |l| self.progress(g, l.node)),
            Node::Mirror { child, .. } => self.progress(g, *child),
            Node::Slot(slot) => self.slots[slot.index()]
                .playing(g)
                .map_or(0.0, |p| p.progress),
            Node::Blend(_) | Node::Pass | Node::Bones(_) => 0.0,
        }
    }

    fn set_state_vars(&mut self, g: &Graph, time: f32, progress: f32) -> (f32, f32) {
        let b = g.vars.builtins;
        let old = (
            self.vars[b + BUILTIN_STATE_TIME],
            self.vars[b + BUILTIN_STATE_PROGRESS],
        );
        self.vars[b + BUILTIN_STATE_TIME] = time;
        self.vars[b + BUILTIN_STATE_PROGRESS] = progress;
        old
    }

    /// Record every marker of `clip` in `[from, to)` on the unwrapped timeline.
    pub(super) fn cross(
        &mut self,
        g: &Graph,
        clip: ClipId,
        from: f32,
        to: f32,
        looping: bool,
        mirrored: bool,
    ) {
        if to.is_nan() || from.is_nan() || to <= from {
            return;
        }
        let anim = g.clips.get(clip);
        let markers = anim.markers();
        if markers.is_empty() {
            return;
        }
        let len = anim.length;
        let fired = |marker| FiredMarker {
            clip,
            marker,
            mirrored,
        };
        if looping && len > 0.0 {
            let first = (from / len).floor();
            let cycles = ((to / len).floor() - first).clamp(0.0, 4.0) as usize;
            for k in 0..=cycles {
                let base = (first + k as f32) * len;
                for (i, m) in markers.iter().enumerate() {
                    let at = base + m.time;
                    if at >= from && at < to {
                        self.markers.push(fired(i));
                    }
                }
            }
        } else {
            for (i, m) in markers.iter().enumerate() {
                if m.time >= from && m.time < to {
                    self.markers.push(fired(i));
                }
            }
        }
    }
}

fn sample_time(t: f32, len: f32, looping: bool) -> f32 {
    if len <= 0.0 {
        0.0
    } else if looping {
        t.rem_euclid(len)
    } else {
        t.clamp(0.0, len)
    }
}

/// The two points `x` falls between and how far from the first.
fn locate(points: &[(f32, NodeId)], x: f32) -> (NodeId, NodeId, f32) {
    let (first, last) = (points[0], points[points.len() - 1]);
    if !x.is_finite() || x <= first.0 {
        return (first.1, first.1, 0.0);
    }
    if x >= last.0 {
        return (last.1, last.1, 0.0);
    }
    let k = points.partition_point(|(at, _)| *at <= x);
    let (a, b) = (points[k - 1], points[k]);
    (a.1, b.1, (x - a.0) / (b.0 - a.0))
}
