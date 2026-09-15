//! What a body publishes to its animator, shared by every player animator
//! driver (the first-person viewmodel and the body): the driver's own
//! per-body param table, the `hurt` edge, the two hands' inputs and the
//! claim driver, in the one frame order both must keep — release last
//! frame's claims, write the engine's inputs, write this frame's claims.
//!
//! Per hand (`main.` / `off.`) the params `held mining eating eat swing_claim
//! jab_claim`, the DRAWN item's `kind` (its render kind's name) and the held
//! stack's `item tool food` (interned strings), and the LEVEL-derived events
//! `equip mine eat` (`mine` and `eat` on the level's rising edge, `equip` on
//! an item change after the first frame). A graph declares the inputs it
//! reads; nothing else is published. The one-shot gestures (`swing break
//! place interact throw`) are not levels: they arrive as resolved graph
//! events through `AnimatorInputs::events`.
//!
//! `swing_claim` / `jab_claim` are published at their released value (0)
//! every frame: a graph's gates read them, and a mod that animates a hand
//! itself sets them through the animator params, which write after these
//! inputs, so its value wins while its claim stands.

use std::sync::Arc;

use petramond::player::RigId;
use petramond_world::animation::expr::intern;
use petramond_world::animation::{Animator, EventId, Graph, ParamId};
use petramond_world::item::ItemType;

use crate::animator_claims::ClaimDriver;
use crate::{AnimatorInputs, HeldItemFrame};

type HandInput = fn(&HeldItemFrame) -> f32;

/// One of a driver's own body params, read off its motion `M`.
type Input<M> = fn(&M) -> f32;

/// A body's motion as its driver reads it: the param table reads the rest.
pub(crate) trait BodyMotion: Copy {
    /// Seconds of hurt left; a rise is a fresh hit.
    fn hurt(&self) -> f32;
}

const HAND: &[(&str, HandInput)] = &[
    ("held", |f| flag(f.item.is_some())),
    ("mining", |f| flag(f.mining)),
    ("eating", |f| flag(f.eating.is_some())),
    ("eat", |f| f.eating.unwrap_or(0.0)),
    ("swing_claim", |_| 0.0),
    ("jab_claim", |_| 0.0),
];

/// The hold the fist takes follows the item whose art it carries, so a
/// display stand-in rests in the fist like what it looks like.
const KIND: &str = "kind";

/// Facts about the held stack itself: a display changes only the look.
const ITEM: [&str; 3] = ["item", "tool", "food"];

pub(crate) fn flag(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}

fn kind_fact(item: Option<ItemType>) -> f32 {
    intern(item.map_or("none", |item| item.render_kind().name()))
}

fn item_facts(item: Option<ItemType>) -> [f32; 3] {
    let Some(item) = item else {
        let none = intern("none");
        return [none, none, 0.0];
    };
    let tool = item.tool().map_or("none", |tool| tool.kind.name());
    [
        intern(item.key()),
        intern(tool),
        flag(item.food().is_some()),
    ]
}

/// One hand's resolved inputs on one graph, and the edges it tracks.
pub(crate) struct HandInputs {
    params: Vec<(ParamId, HandInput)>,
    kind: Option<ParamId>,
    item: [Option<ParamId>; 3],
    equip: Option<EventId>,
    mine: Option<EventId>,
    eat: Option<EventId>,
    /// The held stack the equip edge and the item facts last saw; `None`
    /// inside is an empty hand, `None` outside means nothing seen yet.
    held: Option<Option<ItemType>>,
    /// The drawn item the kind was last resolved for.
    drawn: Option<Option<ItemType>>,
    /// The facts, interned when their item changes and written every frame
    /// like every other engine input, so a released claim on one uncovers
    /// the engine's value.
    kind_value: f32,
    item_values: [f32; 3],
    was_mining: bool,
    was_eating: bool,
}

impl HandInputs {
    /// The inputs `graph` declares under `prefix` (`"main"` or `"off"`).
    pub fn new(graph: &Graph, prefix: &str) -> Self {
        Self {
            params: HAND
                .iter()
                .filter_map(|(name, input)| {
                    graph
                        .param(&format!("{prefix}.{name}"))
                        .map(|id| (id, *input))
                })
                .collect(),
            kind: graph.param(&format!("{prefix}.{KIND}")),
            item: ITEM.map(|name| graph.param(&format!("{prefix}.{name}"))),
            equip: graph.event(&format!("{prefix}.equip")),
            mine: graph.event(&format!("{prefix}.mine")),
            eat: graph.event(&format!("{prefix}.eat")),
            held: None,
            drawn: None,
            kind_value: 0.0,
            item_values: [0.0; 3],
            was_mining: false,
            was_eating: false,
        }
    }

    pub fn reset(&mut self) {
        self.held = None;
        self.drawn = None;
        self.was_mining = false;
        self.was_eating = false;
    }

    pub fn publish(&mut self, animator: &mut Animator, frame: &HeldItemFrame) {
        for (id, input) in &self.params {
            animator.set(*id, input(frame));
        }
        if self.held != Some(frame.item) {
            if self.held.is_some() {
                if let Some(event) = self.equip {
                    animator.fire(event);
                }
            }
            self.held = Some(frame.item);
            self.item_values = item_facts(frame.item);
        }
        let drawn = frame.display.or(frame.item);
        if self.drawn != Some(drawn) {
            self.drawn = Some(drawn);
            self.kind_value = kind_fact(drawn);
        }
        if let Some(id) = self.kind {
            animator.set(id, self.kind_value);
        }
        for (id, value) in self.item.iter().zip(self.item_values) {
            if let Some(id) = id {
                animator.set(*id, value);
            }
        }
        let (mining, eating) = (frame.mining, frame.eating.is_some());
        for (event, happened) in [
            (self.mine, mining && !self.was_mining),
            (self.eat, eating && !self.was_eating),
        ] {
            if let (Some(event), true) = (event, happened) {
                animator.fire(event);
            }
        }
        self.was_mining = mining;
        self.was_eating = eating;
    }
}

/// The core every player animator driver embeds: the animator, the body's
/// own param table over its motion `M`, the `hurt` edge, both hands and the
/// claim driver. The two drivers differ only in their motion type and table.
pub(crate) struct BodyDriver<M> {
    pub animator: Animator,
    body: Vec<(ParamId, Input<M>)>,
    hands: [HandInputs; 2],
    claims: ClaimDriver,
    hurt: Option<EventId>,
    last_hurt: f32,
    /// The last motion published, which a frame with no body to read keeps.
    last: Option<M>,
}

impl<M: BodyMotion> BodyDriver<M> {
    pub fn new(rig: RigId, graph: Arc<Graph>, seed: u32, table: &[(&str, Input<M>)]) -> Self {
        Self {
            body: table
                .iter()
                .filter_map(|(name, input)| graph.param(name).map(|id| (id, *input)))
                .collect(),
            hands: [
                HandInputs::new(&graph, "main"),
                HandInputs::new(&graph, "off"),
            ],
            claims: ClaimDriver::new(rig, &graph),
            hurt: graph.event("hurt"),
            last_hurt: 0.0,
            last: None,
            animator: Animator::new(graph, seed),
        }
    }

    pub fn reset(&mut self) {
        self.animator.reset();
        for hand in &mut self.hands {
            hand.reset();
        }
        self.claims.reset();
        self.last_hurt = 0.0;
        self.last = None;
    }

    /// Open the frame: release last frame's claims, then publish the body's
    /// params and its `hurt` edge (a rise is a fresh hit) from `motion` — or,
    /// on a frame with no body to read (`None`), from the last motion seen,
    /// so a released claim still uncovers the engine's value.
    pub fn begin(&mut self, motion: Option<&M>) {
        self.claims.release(&mut self.animator);
        if let Some(motion) = motion {
            self.last = Some(*motion);
        }
        let Some(motion) = self.last else {
            return;
        };
        for (id, input) in &self.body {
            self.animator.set(*id, input(&motion));
        }
        let hurt = motion.hurt();
        if hurt > self.last_hurt + 1e-4 {
            if let Some(event) = self.hurt {
                self.animator.fire(event);
            }
        }
        self.last_hurt = hurt;
    }

    pub fn publish_hands(&mut self, frames: &[HeldItemFrame; 2]) {
        for (hand, frame) in self.hands.iter_mut().zip(frames) {
            hand.publish(&mut self.animator, frame);
        }
    }

    /// Close the frame's inputs: this frame's claims and fired events, after
    /// everything the engine wrote.
    pub fn claim(&mut self, inputs: AnimatorInputs<'_>) {
        self.claims.claim(&mut self.animator, inputs);
    }
}

#[cfg(test)]
mod tests;
