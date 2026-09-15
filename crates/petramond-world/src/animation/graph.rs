//! Animator graphs: the JSON a rig's animation behaviour is authored in,
//! compiled against the rig's bones and clip library so every name is checked
//! at load.
//!
//! A graph is a tree of pose NODES under a root layer stack:
//!
//! - a CLIP plays one clip (`"walk"`, or `{ "clip": "walk", "rate": "speed / 4.3" }`),
//!   scrubbed instead by `"time"` (seconds) or `"progress"` (0..1) formulas;
//! - a BLEND picks between child nodes along a formula (`"blend"` + `"points"`),
//!   optionally phase-SYNCED so gaits of different lengths keep their feet;
//! - a MACHINE (`"states"`) switches between child nodes on transition
//!   conditions, cross-fading or inertializing between them;
//! - LAYERS stack child nodes, each overriding or adding, weighted by a
//!   formula and limited to a bone mask;
//! - a SLOT plays the one-shot montages events and code start;
//! - BONES drives channels straight from formulas (procedural sway, springs);
//! - MIRROR reflects its child left↔right while a formula holds;
//! - `null` passes the pose beneath through unchanged.
//!
//! Nodes write whole poses over a GROUND: the pose beneath them in an
//! override layer, rest in an additive one. A clip replaces only the channels
//! it keys, so an arm clip over a walk leaves the legs walking.
//!
//! RULES turn named events into montages: play a clip (or pick from several),
//! chain follow-up segments that loop while a formula holds, stop a slot, or
//! freeze a slot for hit-stop. A rule's clip may be a TEMPLATE naming one
//! param (`"fp_swing_{main.tool}"`): the param's value picks the clip when the
//! rule fires, and a value that names no clip makes the rule no match, so
//! the next rule gets the event.
//!
//! GATES stand rules down in one place: a gate names events — and optionally
//! the slots their rules play into — plus a formula, and every rule it names,
//! the document's own or a pack's, fires only while the formula holds.
//!
//! MARKERS map clip marker names to events: a clip crossing a mapped marker
//! fires the event on the next update. A mirrored clip reads its markers
//! under the other side's names (`left_foot` fires what `right_foot` maps).

use rustc_hash::FxHashMap;

use super::expr::Expr;
use super::library::{ClipId, ClipLibrary};
use super::pose::MirrorMap;
use crate::bbmodel::{Channel, Model};

mod parse;

macro_rules! graph_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(pub(crate) u16);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }

            /// The id at `index` in its graph — for ids carried as plain
            /// numbers (a wire row, a name table).
            pub fn from_index(index: u16) -> Self {
                $name(index)
            }
        }
    };
}

graph_id!(
    /// A graph param, in name order (`params` is an object; its keys sort).
    ParamId
);
graph_id!(
    /// A graph event, in `events` list order.
    EventId
);
graph_id!(
    /// A montage slot, in `slots` list order.
    SlotId
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NodeId(pub(crate) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExprId(pub(crate) u32);

/// How a fade's 0..1 progress maps to blend weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ease {
    Linear,
    #[default]
    Smooth,
    In,
    Out,
    InOut,
}

impl Ease {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::Smooth => t * t * (3.0 - 2.0 * t),
            Ease::In => t * t,
            Ease::Out => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::InOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

/// Where the expression inputs live in an animator's variable array.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VarLayout {
    pub events: usize,
    pub slots: usize,
    pub builtins: usize,
    pub len: usize,
}

/// Per slot: weight, time, progress, playing clip.
pub(crate) const SLOT_VARS: usize = 4;
pub(crate) const BUILTIN_DT: usize = 0;
pub(crate) const BUILTIN_TIME: usize = 1;
pub(crate) const BUILTIN_STATE_TIME: usize = 2;
pub(crate) const BUILTIN_STATE_PROGRESS: usize = 3;
pub(crate) const BUILTINS: [&str; 4] = ["anim.dt", "anim.time", "state.time", "state.progress"];

pub struct Graph {
    pub(crate) bones: usize,
    pub(crate) clips: ClipLibrary,
    pub(crate) mirror: MirrorMap,
    pub(crate) params: Vec<(String, f32)>,
    pub(crate) events: Vec<String>,
    pub(crate) slots: Vec<String>,
    pub(crate) masks: Vec<Box<[f32]>>,
    pub(crate) exprs: Vec<Expr>,
    pub(crate) nodes: Vec<Node>,
    pub(crate) root: NodeId,
    pub(crate) rules: Vec<Rule>,
    pub(crate) gates: Vec<Gate>,
    pub(crate) templates: Vec<ClipTemplate>,
    /// Per clip, `[as authored, mirrored]`: the event each of its markers
    /// fires. Empty when the document maps no markers.
    pub(crate) marker_events: Vec<[Box<[Option<EventId>]>; 2]>,
    pub(crate) vars: VarLayout,
    pub(crate) expr_state: usize,
    pub(crate) clip_nodes: usize,
    pub(crate) blend_nodes: usize,
    pub(crate) machines: usize,
    pub(crate) param_ids: FxHashMap<String, ParamId>,
}

pub(crate) enum Node {
    Pass,
    Clip(ClipNode),
    Blend(BlendNode),
    Machine(MachineNode),
    Layers(Vec<Layer>),
    Slot(SlotId),
    Bones(Vec<BoneDrive>),
    Mirror { child: NodeId, when: ExprId },
}

pub(crate) struct ClipNode {
    pub clip: ClipId,
    pub time: ClipTime,
    pub looping: bool,
    /// This node's index into the runtime's clip clocks.
    pub clock: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum ClipTime {
    Rate(ExprId),
    Seconds(ExprId),
    Progress(ExprId),
}

pub(crate) struct BlendNode {
    pub by: ExprId,
    /// Ascending thresholds.
    pub points: Vec<(f32, NodeId)>,
    /// Phase-synced children advance one shared phase at this rate formula.
    pub sync: Option<ExprId>,
    /// This node's index into the runtime's blend phases.
    pub phase: usize,
}

pub(crate) struct MachineNode {
    pub index: usize,
    pub initial: usize,
    pub states: Vec<(String, NodeId)>,
    pub transitions: Vec<Transition>,
}

pub(crate) struct Transition {
    /// `None` = from any state other than `to`.
    pub from: Option<usize>,
    pub to: usize,
    pub when: ExprId,
    pub fade: f32,
    pub ease: Ease,
    pub inertial: bool,
}

pub(crate) struct Layer {
    pub node: NodeId,
    pub additive: bool,
    pub weight: ExprId,
    pub mask: Option<usize>,
}

pub(crate) struct BoneDrive {
    pub bone: usize,
    pub channel: Channel,
    pub value: [ExprId; 3],
}

pub(crate) struct Rule {
    pub event: EventId,
    pub when: ExprId,
    /// The gates that name this rule; it fires only while all are open.
    pub gates: Vec<usize>,
    pub cooldown: f32,
    /// Keep matching later rules for the same event after this one fires.
    pub fallthrough: bool,
    pub play: Option<PlayRule>,
    pub stop: Option<(SlotId, f32)>,
    /// The document's `hitstop`: freeze a slot's clips for this many seconds.
    pub freeze: Option<(SlotId, f32)>,
}

/// A gate's formula; which rules it names is resolved into each rule.
pub(crate) struct Gate {
    pub when: ExprId,
}

pub(crate) struct PlayRule {
    pub slot: SlotId,
    /// Alternatives for the first segment.
    pub first: Vec<SegmentDef>,
    pub pick: Pick,
    pub then: Vec<SegmentDef>,
    pub fade_in: f32,
    pub fade_out: f32,
    pub ease: Ease,
    pub inertial: bool,
    pub rate: ExprId,
    pub duration: Option<ExprId>,
    pub mirror: ExprId,
    pub priority: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    /// In order, back to the first once `reset` seconds pass without a fire.
    Cycle { reset: f32 },
    /// At random, never the same clip twice running when there is a choice.
    Random,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SegmentDef {
    pub clip: ClipRef,
    pub until: Until,
    pub rate: Option<ExprId>,
}

/// A rule segment's clip: named outright, or a [`ClipTemplate`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ClipRef {
    Fixed(ClipId),
    Template(usize),
}

/// A clip name with one param in it (`fp_swing_{main.tool}`), resolved at
/// compile into every clip it can name.
pub(crate) struct ClipTemplate {
    /// The param's index in the animator's variables.
    pub param: usize,
    /// The interned value's bits → the clip that value names.
    pub clips: FxHashMap<u32, ClipId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Until {
    /// Plays through once.
    Once,
    /// Loops while the formula holds, then leaves now or at the cycle's end.
    While { cond: ExprId, finish_cycle: bool },
    /// Loops until the slot is stopped.
    Forever,
}

impl Graph {
    /// Compile `source` against `rig`'s bones, taking ownership of the clip
    /// library the graph names clips from.
    pub fn compile(source: &str, rig: &Model, clips: ClipLibrary) -> Result<Graph, String> {
        parse::compile(source, rig, clips)
    }

    pub fn param(&self, name: &str) -> Option<ParamId> {
        self.param_ids.get(name).copied()
    }

    pub fn event(&self, name: &str) -> Option<EventId> {
        declared(&self.events, name).map(EventId)
    }

    pub fn slot(&self, name: &str) -> Option<SlotId> {
        declared(&self.slots, name).map(SlotId)
    }

    pub fn clips(&self) -> &ClipLibrary {
        &self.clips
    }

    pub fn bones(&self) -> usize {
        self.bones
    }

    pub fn mirror_map(&self) -> &MirrorMap {
        &self.mirror
    }

    pub fn param_names(&self) -> impl Iterator<Item = &str> {
        self.params.iter().map(|(n, _)| n.as_str())
    }

    pub fn event_names(&self) -> &[String] {
        &self.events
    }

    pub fn slot_names(&self) -> &[String] {
        &self.slots
    }

    /// The value a param holds when nothing sets it.
    pub fn param_default(&self, param: ParamId) -> f32 {
        self.params.get(param.index()).map_or(0.0, |(_, v)| *v)
    }

    /// The event marker `marker` of `clip` fires, if the document maps one;
    /// `mirrored` reads the marker under its other side's name.
    pub fn marker_event(&self, clip: ClipId, marker: usize, mirrored: bool) -> Option<EventId> {
        self.marker_events.get(clip.index())?[usize::from(mirrored)]
            .get(marker)
            .copied()
            .flatten()
    }
}

/// The id of `name` in a declared name list.
fn declared(names: &[String], name: &str) -> Option<u16> {
    names.iter().position(|n| n == name).map(|i| i as u16)
}

#[cfg(test)]
mod tests;
