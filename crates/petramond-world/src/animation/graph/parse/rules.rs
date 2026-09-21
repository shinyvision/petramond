//! The event half of a graph document: rules turn events into montages,
//! gates stand rules down, markers turn clip markers into events.

use rustc_hash::FxHashMap;
use serde_json::Value;

use super::{ease, inertial, number, only_keys, Compiler, Obj};
use crate::animation::expr::intern;
use crate::animation::graph::{
    declared, ClipRef, ClipTemplate, EventId, ExprId, Pick, PlayRule, Rule, SegmentDef, SlotId,
    Until,
};
use crate::animation::pose::swap_side;

const RULE_KEYS: &[&str] = &[
    "id",
    "on",
    "when",
    "cooldown",
    "fallthrough",
    "slot",
    "play",
    "pick",
    "reset",
    "then",
    "fade_in",
    "fade_out",
    "ease",
    "blend",
    "rate",
    "duration",
    "mirror",
    "priority",
    "stop",
    "hitstop",
];
const PLAY_ONLY_KEYS: &[&str] = &[
    "pick", "reset", "then", "fade_in", "ease", "blend", "rate", "duration", "mirror", "priority",
];
const GATE_KEYS: &[&str] = &["id", "on", "slot", "when"];

/// One gate as compiled: the events and slots it names, and its formula.
pub(super) struct GateDef {
    pub events: Vec<EventId>,
    pub slots: Vec<SlotId>,
    pub when: ExprId,
}

/// The per-clip marker events for [`Graph::marker_events`](crate::animation::graph::Graph).
pub(super) type MarkerEvents = Vec<[Box<[Option<EventId>]>; 2]>;

impl Compiler<'_> {
    /// `markers`: `{ marker name: event }`, resolved into every clip's markers.
    pub(super) fn markers(&self, value: Option<&Value>) -> Result<MarkerEvents, String> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let map = value
            .as_object()
            .ok_or("markers: expected an object of marker name → event")?;
        let mut events: FxHashMap<&str, EventId> = FxHashMap::default();
        for (marker, event) in map {
            let name = event
                .as_str()
                .ok_or_else(|| format!("markers.{marker}: expected an event name"))?;
            let id = declared(&self.events, name)
                .map(EventId)
                .ok_or_else(|| format!("markers.{marker}: `{name}` is not a declared event"))?;
            events.insert(marker.as_str(), id);
        }
        Ok(self
            .clips
            .iter()
            .map(|(_, _, anim)| {
                let read = |mirrored: bool| -> Box<[Option<EventId>]> {
                    anim.markers()
                        .iter()
                        .map(|m| {
                            let swapped = mirrored.then(|| swap_side(&m.name)).flatten();
                            events.get(swapped.as_deref().unwrap_or(&m.name)).copied()
                        })
                        .collect()
                };
                [read(false), read(true)]
            })
            .collect())
    }

    /// `gates`: `[{ "on": event(s), "slot": slot(s)?, "when": formula }]`.
    pub(super) fn gates(&mut self, value: Option<&Value>) -> Result<(), String> {
        let Some(value) = value else {
            return Ok(());
        };
        let list = value.as_array().ok_or("gates: expected a list")?;
        for (i, gate) in list.iter().enumerate() {
            let o = gate
                .as_object()
                .ok_or_else(|| format!("gates[{i}]: expected a gate object"))?;
            let path = match o.get("id").and_then(Value::as_str) {
                Some(id) => format!("gates[{id}]"),
                None => format!("gates[{i}]"),
            };
            only_keys(o, GATE_KEYS, &path)?;
            let events = one_or_many(o.get("on"), &format!("{path}.on"))?
                .into_iter()
                .map(|name| {
                    declared(&self.events, name)
                        .map(EventId)
                        .ok_or_else(|| format!("{path}.on: `{name}` is not a declared event"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if events.is_empty() {
                return Err(format!("{path}: `on` names at least one event"));
            }
            let slots = match o.get("slot") {
                None => Vec::new(),
                Some(v) => one_or_many(Some(v), &format!("{path}.slot"))?
                    .into_iter()
                    .map(|name| {
                        declared(&self.slots, name)
                            .map(SlotId)
                            .ok_or_else(|| format!("{path}.slot: `{name}` is not a declared slot"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            };
            if !o.contains_key("when") {
                return Err(format!("{path}: a gate needs `when`"));
            }
            let when = self.expr(o.get("when"), 1.0, &format!("{path}.when"))?;
            self.gates.push(GateDef {
                events,
                slots,
                when,
            });
        }
        Ok(())
    }

    pub(super) fn rule(&mut self, value: &Value, path: &str) -> Result<Rule, String> {
        let o = value
            .as_object()
            .ok_or_else(|| format!("{path}: expected a rule object"))?;
        only_keys(o, RULE_KEYS, path)?;
        let on = o
            .get("on")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path}: `on` names the event"))?;
        let event = declared(&self.events, on)
            .map(EventId)
            .ok_or_else(|| format!("{path}.on: `{on}` is not a declared event"))?;
        let when = self.expr(o.get("when"), 1.0, &format!("{path}.when"))?;
        let cooldown = number(o.get("cooldown"), 0.0, &format!("{path}.cooldown"))?;
        let fallthrough = match o.get("fallthrough") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(format!("{path}.fallthrough: expected true or false")),
        };
        let slot = match o.get("slot") {
            Some(slot) => Some(self.slot(Some(slot), &format!("{path}.slot"))?),
            None => None,
        };
        let gates = self
            .gates
            .iter()
            .enumerate()
            .filter(|(_, gate)| {
                gate.events.contains(&event)
                    && (gate.slots.is_empty() || slot.is_some_and(|s| gate.slots.contains(&s)))
            })
            .map(|(i, _)| i)
            .collect();
        let fade_out = number(o.get("fade_out"), 0.15, &format!("{path}.fade_out"))?.max(0.0);

        let play = match o.get("play") {
            Some(play) => Some(self.play_rule(o, play, slot, fade_out, path)?),
            None => {
                if let Some(key) = PLAY_ONLY_KEYS.iter().find(|k| o.contains_key(**k)) {
                    return Err(format!("{path}: `{key}` only applies to `play`"));
                }
                None
            }
        };
        let stop = match o.get("stop") {
            Some(stopped) => Some((self.slot(Some(stopped), &format!("{path}.stop"))?, fade_out)),
            None => None,
        };
        let freeze = match o.get("hitstop") {
            Some(seconds) => {
                let slot = slot.ok_or_else(|| format!("{path}: `hitstop` freezes a `slot`"))?;
                Some((
                    slot,
                    number(Some(seconds), 0.0, &format!("{path}.hitstop"))?,
                ))
            }
            None => None,
        };
        if play.is_none() && stop.is_none() && freeze.is_none() {
            return Err(format!("{path}: a rule does `play`, `stop` or `hitstop`"));
        }
        Ok(Rule {
            event,
            when,
            gates,
            cooldown,
            fallthrough,
            play,
            stop,
            freeze,
        })
    }

    fn play_rule(
        &mut self,
        o: &Obj,
        play: &Value,
        slot: Option<SlotId>,
        fade_out: f32,
        path: &str,
    ) -> Result<PlayRule, String> {
        let slot = slot.ok_or_else(|| format!("{path}: `play` needs a `slot`"))?;
        let first = match play {
            Value::Array(list) if list.is_empty() => {
                return Err(format!("{path}.play: an empty list plays nothing"))
            }
            Value::Array(list) => {
                let mut first = Vec::new();
                for (i, s) in list.iter().enumerate() {
                    first.push(self.segment(s, &format!("{path}.play[{i}]"))?);
                }
                first
            }
            single => vec![self.segment(single, &format!("{path}.play"))?],
        };
        let pick = match o.get("pick") {
            None => Pick::Cycle {
                reset: number(o.get("reset"), 1.0, &format!("{path}.reset"))?,
            },
            Some(Value::String(s)) if s == "cycle" => Pick::Cycle {
                reset: number(o.get("reset"), 1.0, &format!("{path}.reset"))?,
            },
            Some(Value::String(s)) if s == "random" => Pick::Random,
            Some(other) => {
                return Err(format!(
                    "{path}.pick: `{other}` is neither cycle nor random"
                ))
            }
        };
        let then = match o.get("then") {
            None => Vec::new(),
            Some(Value::Array(list)) => {
                let mut then = Vec::new();
                for (i, s) in list.iter().enumerate() {
                    then.push(self.segment(s, &format!("{path}.then[{i}]"))?);
                }
                then
            }
            Some(single) => vec![self.segment(single, &format!("{path}.then"))?],
        };
        let duration = match o.get("duration") {
            Some(d) => Some(self.expr(Some(d), 0.0, &format!("{path}.duration"))?),
            None => None,
        };
        let priority = match o.get("priority") {
            None => 0,
            Some(p) => p
                .as_i64()
                .ok_or_else(|| format!("{path}.priority: expected a whole number"))?
                as i32,
        };
        Ok(PlayRule {
            slot,
            first,
            pick,
            then,
            fade_in: number(o.get("fade_in"), 0.1, &format!("{path}.fade_in"))?.max(0.0),
            fade_out,
            ease: ease(o.get("ease"), &format!("{path}.ease"))?,
            inertial: inertial(o.get("blend"), &format!("{path}.blend"))?,
            rate: self.expr(o.get("rate"), 1.0, &format!("{path}.rate"))?,
            duration,
            mirror: self.expr(o.get("mirror"), 0.0, &format!("{path}.mirror"))?,
            priority,
        })
    }

    fn segment(&mut self, value: &Value, path: &str) -> Result<SegmentDef, String> {
        let o = match value {
            Value::String(name) => {
                return Ok(SegmentDef {
                    clip: self.clip_ref(name, path)?,
                    until: Until::Once,
                    rate: None,
                })
            }
            Value::Object(o) => o,
            _ => return Err(format!("{path}: expected a clip name or a segment object")),
        };
        only_keys(o, &["clip", "while", "exit", "loop", "rate"], path)?;
        let name = o
            .get("clip")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path}: a segment names its `clip`"))?;
        let clip = self.clip_ref(name, &format!("{path}.clip"))?;
        let until = match (o.get("while"), o.get("loop")) {
            (Some(_), Some(_)) => {
                return Err(format!("{path}: `while` and `loop` cannot both be set"))
            }
            (Some(cond), None) => {
                let finish_cycle = match o.get("exit").and_then(Value::as_str) {
                    None | Some("finish") => true,
                    Some("now") => false,
                    Some(other) => {
                        return Err(format!("{path}.exit: `{other}` is neither finish nor now"))
                    }
                };
                Until::While {
                    cond: self.expr(Some(cond), 0.0, &format!("{path}.while"))?,
                    finish_cycle,
                }
            }
            (None, loop_) => {
                if o.contains_key("exit") {
                    return Err(format!("{path}.exit: only a `while` segment exits"));
                }
                match loop_ {
                    None | Some(Value::Bool(false)) => Until::Once,
                    Some(Value::Bool(true)) => Until::Forever,
                    Some(_) => return Err(format!("{path}.loop: expected true or false")),
                }
            }
        };
        let rate = match o.get("rate") {
            Some(rate) => Some(self.expr(Some(rate), 1.0, &format!("{path}.rate"))?),
            None => None,
        };
        Ok(SegmentDef { clip, until, rate })
    }

    /// A segment's clip: a plain name, or a template naming one declared
    /// param in braces, resolved now into every clip its values can name.
    fn clip_ref(&mut self, name: &str, path: &str) -> Result<ClipRef, String> {
        let Some(open) = name.find('{') else {
            return Ok(ClipRef::Fixed(self.clip(name, path)?));
        };
        let close = name[open..]
            .find('}')
            .map(|i| open + i)
            .ok_or_else(|| format!("{path}: `{name}` opens a `{{` it never closes"))?;
        let (prefix, param, suffix) = (&name[..open], &name[open + 1..close], &name[close + 1..]);
        if prefix.contains('}') || suffix.contains(['{', '}']) {
            return Err(format!("{path}: `{name}`: a clip template names one param"));
        }
        let index = self
            .params
            .iter()
            .position(|(n, _)| n == param)
            .ok_or_else(|| format!("{path}: `{name}` names no declared param `{param}`"))?;
        let mut clips = FxHashMap::default();
        for (id, full, _) in self.clips.iter() {
            if full.len() > prefix.len() + suffix.len()
                && full.starts_with(prefix)
                && full.ends_with(suffix)
            {
                let value = &full[prefix.len()..full.len() - suffix.len()];
                clips.insert(intern(value).to_bits(), id);
            }
        }
        if clips.is_empty() {
            return Err(format!("{path}: no clip matches `{name}`"));
        }
        self.templates.push(ClipTemplate {
            param: index,
            clips,
        });
        Ok(ClipRef::Template(self.templates.len() - 1))
    }
}

/// A name or a list of names.
fn one_or_many<'a>(value: Option<&'a Value>, path: &str) -> Result<Vec<&'a str>, String> {
    match value {
        Some(Value::String(name)) => Ok(vec![name]),
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| v.as_str().ok_or_else(|| format!("{path}: expected names")))
            .collect(),
        _ => Err(format!("{path}: expected a name or a list of names")),
    }
}
