use rustc_hash::FxHashMap;
use serde_json::{Map, Value};

use super::{
    declared, BlendNode, BoneDrive, ClipNode, ClipTemplate, ClipTime, Ease, ExprId, Gate, Graph,
    Layer, MachineNode, Node, NodeId, ParamId, SlotId, Transition, VarLayout, BUILTINS, SLOT_VARS,
};
use crate::animation::expr::{intern, Expr};
use crate::animation::library::{ClipId, ClipLibrary};
use crate::animation::pose::MirrorMap;
use crate::bbmodel::{Channel, Model};

mod rules;

type Obj = Map<String, Value>;

const DOC_KEYS: &[&str] = &[
    "params", "events", "slots", "masks", "markers", "gates", "layers", "rules",
];
const LAYER_KEYS: &[&str] = &["name", "mode", "weight", "mask"];
const KINDS: &[&str] = &[
    "clip", "blend", "states", "layers", "slot", "bones", "mirror",
];

pub(super) fn compile(source: &str, rig: &Model, clips: ClipLibrary) -> Result<Graph, String> {
    let doc: Value = serde_json::from_str(source).map_err(|e| format!("json: {e}"))?;
    let root = doc
        .as_object()
        .ok_or("an animator graph is a JSON object")?;
    only_keys(root, DOC_KEYS, "graph")?;

    let mut c = Compiler {
        rig,
        clips: &clips,
        params: Vec::new(),
        events: Vec::new(),
        slots: Vec::new(),
        masks: Vec::new(),
        mask_names: FxHashMap::default(),
        vars: FxHashMap::default(),
        layout: VarLayout {
            events: 0,
            slots: 0,
            builtins: 0,
            len: 0,
        },
        exprs: Vec::new(),
        expr_state: 0,
        nodes: Vec::new(),
        clip_nodes: 0,
        blend_nodes: 0,
        machines: 0,
        gates: Vec::new(),
        templates: Vec::new(),
    };
    c.declare(root)?;
    let marker_events = c.markers(root.get("markers"))?;
    c.gates(root.get("gates"))?;
    let layers = root.get("layers").ok_or("graph: `layers` is required")?;
    let root_node = c.layers(layers, "layers")?;
    let mut rules = Vec::new();
    if let Some(list) = root.get("rules") {
        let list = list.as_array().ok_or("rules: expected a list")?;
        for (i, rule) in list.iter().enumerate() {
            rules.push(c.rule(rule, &format!("rules[{i}]"))?);
        }
    }

    let Compiler {
        params,
        events,
        slots,
        masks,
        layout,
        exprs,
        expr_state,
        nodes,
        clip_nodes,
        blend_nodes,
        machines,
        gates,
        templates,
        ..
    } = c;
    let param_ids = params
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.clone(), ParamId(i as u16)))
        .collect();
    Ok(Graph {
        bones: rig.bones().len(),
        clips,
        mirror: MirrorMap::for_model(rig),
        params,
        events,
        slots,
        masks,
        exprs,
        nodes,
        root: root_node,
        rules,
        gates: gates
            .into_iter()
            .map(|gate| Gate { when: gate.when })
            .collect(),
        templates,
        marker_events,
        vars: layout,
        expr_state,
        clip_nodes,
        blend_nodes,
        machines,
        param_ids,
    })
}

struct Compiler<'a> {
    rig: &'a Model,
    clips: &'a ClipLibrary,
    params: Vec<(String, f32)>,
    events: Vec<String>,
    slots: Vec<String>,
    masks: Vec<Box<[f32]>>,
    mask_names: FxHashMap<String, usize>,
    vars: FxHashMap<String, u16>,
    layout: VarLayout,
    exprs: Vec<Expr>,
    expr_state: usize,
    nodes: Vec<Node>,
    clip_nodes: usize,
    blend_nodes: usize,
    machines: usize,
    gates: Vec<rules::GateDef>,
    templates: Vec<ClipTemplate>,
}

impl Compiler<'_> {
    fn declare(&mut self, root: &Obj) -> Result<(), String> {
        if let Some(params) = root.get("params") {
            let params = params
                .as_object()
                .ok_or("params: expected an object of name → default")?;
            for (name, value) in params {
                let default = match value {
                    Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
                    Value::Bool(b) => f32::from(u8::from(*b)),
                    Value::String(s) => intern(s),
                    other => {
                        return Err(format!(
                            "params.{name}: a default is a number, a bool or a string, not {other}"
                        ))
                    }
                };
                self.params.push((name.clone(), default));
            }
        }
        self.events = names_list(root.get("events"), "events")?;
        self.slots = names_list(root.get("slots"), "slots")?;

        let p = self.params.len();
        let e = self.events.len();
        let s = self.slots.len();
        self.layout = VarLayout {
            events: p,
            slots: p + e,
            builtins: p + e + s * SLOT_VARS,
            len: p + e + s * SLOT_VARS + BUILTINS.len(),
        };
        if self.layout.len > u16::MAX as usize {
            return Err("graph: too many params, events and slots".into());
        }
        let mut names: Vec<(String, usize)> = Vec::new();
        names.extend(
            self.params
                .iter()
                .enumerate()
                .map(|(i, (n, _))| (n.clone(), i)),
        );
        names.extend(
            self.events
                .iter()
                .enumerate()
                .map(|(i, n)| (format!("event.{n}"), p + i)),
        );
        for (i, n) in self.slots.iter().enumerate() {
            let base = self.layout.slots + i * SLOT_VARS;
            names.push((format!("slot.{n}"), base));
            names.push((format!("slot.{n}.time"), base + 1));
            names.push((format!("slot.{n}.progress"), base + 2));
            names.push((format!("slot.{n}.clip"), base + 3));
        }
        names.extend(
            BUILTINS
                .iter()
                .enumerate()
                .map(|(i, n)| (n.to_string(), self.layout.builtins + i)),
        );
        for (name, index) in names {
            if self.vars.insert(name.clone(), index as u16).is_some() {
                return Err(format!("graph: the name `{name}` is declared twice"));
            }
        }

        if let Some(masks) = root.get("masks") {
            let masks = masks
                .as_object()
                .ok_or("masks: expected an object of name → { bone: weight }")?;
            for (name, mask) in masks {
                let index = self.mask(mask, &format!("masks.{name}"))?;
                self.mask_names.insert(name.clone(), index);
            }
        }
        Ok(())
    }

    /// A mask weights each listed bone AND its descendants; a descendant
    /// listed itself takes its own weight instead.
    fn mask(&mut self, value: &Value, path: &str) -> Result<usize, String> {
        let listed = match value {
            Value::String(name) => {
                return self
                    .mask_names
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("{path}: no mask named `{name}`"))
            }
            Value::Object(listed) => listed,
            _ => {
                return Err(format!(
                    "{path}: expected a mask name or {{ bone: weight }}"
                ))
            }
        };
        let bones = self.rig.bones();
        let mut weights: FxHashMap<usize, f32> = FxHashMap::default();
        for (bone, w) in listed {
            let index = self.bone(bone, path)?;
            let w = w
                .as_f64()
                .ok_or_else(|| format!("{path}.{bone}: a mask weight is a number"))?;
            weights.insert(index, w as f32);
        }
        let per_bone = (0..bones.len())
            .map(|i| {
                let mut at = Some(i);
                for _ in 0..=bones.len() {
                    let Some(b) = at else { break };
                    if let Some(w) = weights.get(&b) {
                        return *w;
                    }
                    at = bones[b].parent.filter(|&p| p != b);
                }
                0.0
            })
            .collect();
        self.masks.push(per_bone);
        Ok(self.masks.len() - 1)
    }

    fn bone(&self, name: &str, path: &str) -> Result<usize, String> {
        self.rig
            .bone_named(name)
            .ok_or_else(|| format!("{path}: the rig has no bone `{name}`"))
    }

    fn clip(&self, name: &str, path: &str) -> Result<ClipId, String> {
        self.clips
            .id(name)
            .ok_or_else(|| format!("{path}: no clip named `{name}`"))
    }

    fn slot(&self, value: Option<&Value>, path: &str) -> Result<SlotId, String> {
        let name = value
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path}: expected a slot name"))?;
        declared(&self.slots, name)
            .map(SlotId)
            .ok_or_else(|| format!("{path}: `{name}` is not a declared slot"))
    }

    fn expr(&mut self, value: Option<&Value>, default: f32, path: &str) -> Result<ExprId, String> {
        let expr = match value {
            None => Expr::constant(default),
            Some(Value::Number(n)) => Expr::constant(n.as_f64().unwrap_or(0.0) as f32),
            Some(Value::Bool(b)) => Expr::constant(f32::from(u8::from(*b))),
            Some(Value::String(source)) => {
                let vars = &self.vars;
                Expr::compile(source, &|name| vars.get(name).copied(), self.expr_state)
                    .map_err(|e| format!("{path}: {e}"))?
            }
            Some(other) => {
                return Err(format!(
                    "{path}: expected a number or a formula, found {other}"
                ))
            }
        };
        self.expr_state += expr.state_slots();
        self.exprs.push(expr);
        Ok(ExprId(self.exprs.len() as u32 - 1))
    }

    fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        NodeId(self.nodes.len() as u32 - 1)
    }

    fn node(&mut self, value: &Value, path: &str) -> Result<NodeId, String> {
        match value {
            Value::Null => Ok(self.push(Node::Pass)),
            Value::String(name) => {
                let clip = self.clip(name, path)?;
                let rate = self.expr(None, 1.0, path)?;
                let looping = self.clips.get(clip).looping;
                Ok(self.clip_node(clip, ClipTime::Rate(rate), looping))
            }
            Value::Object(o) => self.node_object(o, &[], path),
            _ => Err(format!("{path}: expected a clip name, a node or null")),
        }
    }

    fn clip_node(&mut self, clip: ClipId, time: ClipTime, looping: bool) -> NodeId {
        let clock = self.clip_nodes;
        self.clip_nodes += 1;
        self.push(Node::Clip(ClipNode {
            clip,
            time,
            looping,
            clock,
        }))
    }

    fn node_object(&mut self, o: &Obj, extra: &[&str], path: &str) -> Result<NodeId, String> {
        let kinds: Vec<&str> = KINDS
            .iter()
            .copied()
            .filter(|k| o.contains_key(*k))
            .collect();
        let kind = match kinds.as_slice() {
            [kind] => *kind,
            [] => return Err(format!("{path}: a node needs one of: {}", KINDS.join(", "))),
            many => return Err(format!("{path}: one node cannot be {}", many.join(" and "))),
        };
        let allowed: &[&str] = match kind {
            "clip" => &["clip", "rate", "time", "progress", "loop"],
            "blend" => &["blend", "points", "sync", "rate"],
            "states" => &["states", "initial", "transitions"],
            "mirror" => &["mirror", "when"],
            _ => &[],
        };
        if let Some(key) = o
            .keys()
            .find(|k| *k != kind && !allowed.contains(&k.as_str()) && !extra.contains(&k.as_str()))
        {
            return Err(format!("{path}: `{key}` is not a key of a {kind} node"));
        }
        match kind {
            "clip" => self.clip_object(o, path),
            "blend" => self.blend(o, path),
            "states" => self.machine(o, path),
            "layers" => self.layers(&o["layers"], &format!("{path}.layers")),
            "slot" => {
                let slot = self.slot(o.get("slot"), &format!("{path}.slot"))?;
                if self
                    .nodes
                    .iter()
                    .any(|n| matches!(n, Node::Slot(s) if *s == slot))
                {
                    return Err(format!(
                        "{path}.slot: `{}` already plays in another node",
                        self.slots[slot.index()]
                    ));
                }
                Ok(self.push(Node::Slot(slot)))
            }
            "bones" => self.bones(&o["bones"], &format!("{path}.bones")),
            _ => {
                let child = self.node(&o["mirror"], &format!("{path}.mirror"))?;
                let when = self.expr(o.get("when"), 1.0, &format!("{path}.when"))?;
                Ok(self.push(Node::Mirror { child, when }))
            }
        }
    }

    fn clip_object(&mut self, o: &Obj, path: &str) -> Result<NodeId, String> {
        let name = o["clip"]
            .as_str()
            .ok_or_else(|| format!("{path}.clip: expected a clip name"))?;
        let clip = self.clip(name, &format!("{path}.clip"))?;
        let timings = ["rate", "time", "progress"]
            .iter()
            .filter(|k| o.contains_key(**k))
            .count();
        if timings > 1 {
            return Err(format!(
                "{path}: a clip takes one of `rate`, `time` or `progress`"
            ));
        }
        let time = if o.contains_key("time") {
            ClipTime::Seconds(self.expr(o.get("time"), 0.0, &format!("{path}.time"))?)
        } else if o.contains_key("progress") {
            ClipTime::Progress(self.expr(o.get("progress"), 0.0, &format!("{path}.progress"))?)
        } else {
            ClipTime::Rate(self.expr(o.get("rate"), 1.0, &format!("{path}.rate"))?)
        };
        let looping = match o.get("loop") {
            None => self.clips.get(clip).looping,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(format!("{path}.loop: expected true or false")),
        };
        Ok(self.clip_node(clip, time, looping))
    }

    fn blend(&mut self, o: &Obj, path: &str) -> Result<NodeId, String> {
        let by = self.expr(o.get("blend"), 0.0, &format!("{path}.blend"))?;
        let list = o
            .get("points")
            .and_then(Value::as_array)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("{path}: a blend needs `points`: [[threshold, node], ...]"))?;
        let mut points: Vec<(f32, NodeId)> = Vec::new();
        for (i, point) in list.iter().enumerate() {
            let at_path = format!("{path}.points[{i}]");
            let pair = point
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| format!("{at_path}: expected [threshold, node]"))?;
            let at = pair[0]
                .as_f64()
                .ok_or_else(|| format!("{at_path}: a threshold is a number"))?
                as f32;
            if points.last().is_some_and(|(prev, _)| at <= *prev) {
                return Err(format!("{at_path}: thresholds must ascend"));
            }
            let node = self.node(&pair[1], &at_path)?;
            points.push((at, node));
        }
        let sync = match o.get("sync") {
            None | Some(Value::Bool(false)) => {
                if o.contains_key("rate") {
                    return Err(format!(
                        "{path}.rate: only a synced blend has a rate; set it on the clips"
                    ));
                }
                None
            }
            Some(Value::Bool(true)) => {
                for (i, (_, node)) in points.iter().enumerate() {
                    if !matches!(self.nodes[node.0 as usize], Node::Clip(_)) {
                        return Err(format!("{path}.points[{i}]: a synced blend blends clips"));
                    }
                }
                Some(self.expr(o.get("rate"), 1.0, &format!("{path}.rate"))?)
            }
            Some(_) => return Err(format!("{path}.sync: expected true or false")),
        };
        let phase = self.blend_nodes;
        self.blend_nodes += 1;
        Ok(self.push(Node::Blend(BlendNode {
            by,
            points,
            sync,
            phase,
        })))
    }

    fn machine(&mut self, o: &Obj, path: &str) -> Result<NodeId, String> {
        let states_obj = o["states"]
            .as_object()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{path}.states: expected an object of name → node"))?;
        let names: Vec<String> = states_obj.keys().cloned().collect();
        let initial_name = o
            .get("initial")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path}: a machine names its `initial` state"))?;
        let initial = names
            .iter()
            .position(|n| n == initial_name)
            .ok_or_else(|| format!("{path}.initial: no state `{initial_name}`"))?;
        let index = self.machines;
        self.machines += 1;
        let mut states = Vec::new();
        for (name, node) in states_obj {
            let node = self.node(node, &format!("{path}.states.{name}"))?;
            states.push((name.clone(), node));
        }
        let mut transitions = Vec::new();
        if let Some(list) = o.get("transitions") {
            let list = list
                .as_array()
                .ok_or_else(|| format!("{path}.transitions: expected a list"))?;
            for (i, t) in list.iter().enumerate() {
                self.transition(
                    t,
                    &names,
                    &mut transitions,
                    &format!("{path}.transitions[{i}]"),
                )?;
            }
        }
        Ok(self.push(Node::Machine(MachineNode {
            index,
            initial,
            states,
            transitions,
        })))
    }

    fn transition(
        &mut self,
        value: &Value,
        names: &[String],
        out: &mut Vec<Transition>,
        path: &str,
    ) -> Result<(), String> {
        let o = value
            .as_object()
            .ok_or_else(|| format!("{path}: expected a transition object"))?;
        only_keys(o, &["from", "to", "when", "fade", "ease", "blend"], path)?;
        let state = |name: &str| {
            names
                .iter()
                .position(|n| n == name)
                .ok_or_else(|| format!("{path}: no state `{name}`"))
        };
        let to = state(
            o.get("to")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{path}: `to` names a state"))?,
        )?;
        let from: Vec<Option<usize>> = match o.get("from") {
            Some(Value::String(s)) if s == "*" => vec![None],
            Some(Value::String(s)) => vec![Some(state(s)?)],
            Some(Value::Array(list)) => {
                let mut from = Vec::new();
                for s in list {
                    let s = s
                        .as_str()
                        .ok_or_else(|| format!("{path}.from: expected state names"))?;
                    from.push(Some(state(s)?));
                }
                from
            }
            _ => {
                return Err(format!(
                    "{path}: `from` is a state, a list of states, or \"*\""
                ))
            }
        };
        if !o.contains_key("when") {
            return Err(format!("{path}: a transition needs `when`"));
        }
        let when = self.expr(o.get("when"), 0.0, &format!("{path}.when"))?;
        let fade = number(o.get("fade"), 0.2, &format!("{path}.fade"))?.max(0.0);
        let ease = ease(o.get("ease"), &format!("{path}.ease"))?;
        let inertial = inertial(o.get("blend"), &format!("{path}.blend"))?;
        for from in from {
            if from == Some(to) {
                return Err(format!(
                    "{path}: `{}` cannot transition to itself",
                    names[to]
                ));
            }
            out.push(Transition {
                from,
                to,
                when,
                fade,
                ease,
                inertial,
            });
        }
        Ok(())
    }

    fn layers(&mut self, value: &Value, path: &str) -> Result<NodeId, String> {
        let list = value
            .as_array()
            .ok_or_else(|| format!("{path}: expected a list of layers"))?;
        let mut layers = Vec::new();
        for (i, layer) in list.iter().enumerate() {
            let o = layer
                .as_object()
                .ok_or_else(|| format!("{path}[{i}]: expected a layer object"))?;
            let layer_path = match o.get("name").and_then(Value::as_str) {
                Some(name) => format!("{path}[{name}]"),
                None => format!("{path}[{i}]"),
            };
            let additive = match o.get("mode").and_then(Value::as_str) {
                None | Some("override") => false,
                Some("additive") => true,
                Some(other) => {
                    return Err(format!(
                        "{layer_path}.mode: `{other}` is neither override nor additive"
                    ))
                }
            };
            let weight = self.expr(o.get("weight"), 1.0, &format!("{layer_path}.weight"))?;
            let mask = match o.get("mask") {
                Some(mask) => Some(self.mask(mask, &format!("{layer_path}.mask"))?),
                None => None,
            };
            let node = self.node_object(o, LAYER_KEYS, &layer_path)?;
            layers.push(Layer {
                node,
                additive,
                weight,
                mask,
            });
        }
        Ok(self.push(Node::Layers(layers)))
    }

    fn bones(&mut self, value: &Value, path: &str) -> Result<NodeId, String> {
        let bones = value
            .as_object()
            .ok_or_else(|| format!("{path}: expected {{ bone: {{ rotation, position }} }}"))?;
        let mut drives = Vec::new();
        for (name, channels) in bones {
            let bone_path = format!("{path}.{name}");
            let bone = self.bone(name, &bone_path)?;
            let channels = channels
                .as_object()
                .ok_or_else(|| format!("{bone_path}: expected {{ rotation, position }}"))?;
            only_keys(channels, &["rotation", "position"], &bone_path)?;
            for (key, channel) in [
                ("rotation", Channel::Rotation),
                ("position", Channel::Position),
            ] {
                let Some(v) = channels.get(key) else { continue };
                let channel_path = format!("{bone_path}.{key}");
                let xyz = v
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .ok_or_else(|| format!("{channel_path}: expected [x, y, z]"))?;
                let value = [
                    self.expr(Some(&xyz[0]), 0.0, &format!("{channel_path}[0]"))?,
                    self.expr(Some(&xyz[1]), 0.0, &format!("{channel_path}[1]"))?,
                    self.expr(Some(&xyz[2]), 0.0, &format!("{channel_path}[2]"))?,
                ];
                drives.push(BoneDrive {
                    bone,
                    channel,
                    value,
                });
            }
        }
        Ok(self.push(Node::Bones(drives)))
    }
}

fn only_keys(o: &Obj, allowed: &[&str], path: &str) -> Result<(), String> {
    match o.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(format!("{path}: unknown key `{key}`")),
        None => Ok(()),
    }
}

fn names_list(value: Option<&Value>, path: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .ok_or_else(|| format!("{path}: expected a list of names"))?;
    let mut names: Vec<String> = Vec::new();
    for name in list {
        let name = name
            .as_str()
            .ok_or_else(|| format!("{path}: expected a list of names"))?;
        if names.iter().any(|n| n == name) {
            return Err(format!("{path}: `{name}` is listed twice"));
        }
        names.push(name.to_string());
    }
    Ok(names)
}

fn number(value: Option<&Value>, default: f32, path: &str) -> Result<f32, String> {
    match value {
        None => Ok(default),
        Some(v) => v
            .as_f64()
            .map(|n| n as f32)
            .ok_or_else(|| format!("{path}: expected a number")),
    }
}

fn ease(value: Option<&Value>, path: &str) -> Result<Ease, String> {
    Ok(match value.map(|v| v.as_str().unwrap_or("?")) {
        None => Ease::default(),
        Some("linear") => Ease::Linear,
        Some("smooth") => Ease::Smooth,
        Some("in") => Ease::In,
        Some("out") => Ease::Out,
        Some("in_out") => Ease::InOut,
        Some(other) => {
            return Err(format!(
                "{path}: `{other}` is not linear, smooth, in, out or in_out"
            ))
        }
    })
}

fn inertial(value: Option<&Value>, path: &str) -> Result<bool, String> {
    match value.map(|v| v.as_str().unwrap_or("?")) {
        None | Some("crossfade") => Ok(false),
        Some("inertial") => Ok(true),
        Some(other) => Err(format!(
            "{path}: `{other}` is neither crossfade nor inertial"
        )),
    }
}
