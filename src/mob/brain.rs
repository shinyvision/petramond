use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::world::ServerWorld;
pub use mod_api::{AiNodeDecision, ChannelClaims, DecisionChannel};
use petramond_math::math::{IVec3, Vec3};

use super::behavior::ScriptedNode;
use super::model_meta::IdleAnimMeta;
use super::noise::NoiseField;
use super::{EntityRef, Mob, MobRng, PlayerAnchor};

pub const PRIORITY_WANDER: u8 = 0;
pub const PRIORITY_EXPRESSION: u8 = 10;
pub const PRIORITY_CHASE: u8 = 20;
pub const PRIORITY_CONTACT: u8 = 22;
pub const PRIORITY_ATTACK: u8 = 30;
pub const PRIORITY_DAMAGE_RESPONSE: u8 = 40;
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeadLook {
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Debug)]
pub struct AiMob {
    pub id: u64,
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub active: bool,
    pub tags: Arc<BTreeMap<String, super::MobTagValue>>,
}

impl AiMob {
    pub fn bool_tag(&self, key: &str) -> bool {
        self.tags.get(key).and_then(super::MobTagValue::as_bool) == Some(true)
    }

    pub fn int_tag(&self, key: &str) -> Option<i64> {
        self.tags.get(key).and_then(super::MobTagValue::as_int)
    }

    pub fn float_tag(&self, key: &str) -> Option<f64> {
        self.tags.get(key).and_then(super::MobTagValue::as_float)
    }

    pub fn str_tag(&self, key: &str) -> Option<&str> {
        self.tags.get(key).and_then(super::MobTagValue::as_str)
    }

    pub fn confined(&self) -> bool {
        self.bool_tag(super::tags::CONFINED)
    }
}

pub struct TickInputs<'a> {
    pub world: &'a ServerWorld,
    pub players: &'a [PlayerAnchor],
    pub noises: &'a NoiseField,
    pub mobs: &'a super::spatial::MobSnapshot,
    pub solid: &'a [petramond_world::collision::DynBox],
    pub path_budget: Option<&'a super::nav::PathBudget>,
    pub solid_escape: &'a [petramond_world::collision::DynBox],
}

pub struct AiCtx<'a> {
    pub mob_id: u64,
    pub pos: petramond_math::world_pos::WorldPos,
    pub cell: IVec3,
    pub yaw: f32,
    pub head_height: f32,
    pub half_width: f32,
    pub world: &'a ServerWorld,
    pub reach: Option<&'a super::nav::ReachBudget>,
    pub player_id: crate::player::PlayerId,
    pub player_pos: petramond_math::world_pos::WorldPos,
    pub player_sneaking: bool,
    pub player_held: Option<petramond_world::item::ItemType>,
    pub players: &'a [PlayerAnchor],
    pub noises: &'a NoiseField,
    pub contacts: &'a [EntityRef],
    pub target: Option<EntityRef>,
    pub attacker: Option<(EntityRef, u32)>,
    pub nav_idle: bool,
    pub in_fluid: Option<petramond_world::block::Block>,
    pub head: i32,
    pub tolerated: &'static [petramond_world::block::Block],
    pub idle_anims: &'a [IdleAnimMeta],
    pub mob_index: Option<usize>,
    pub mobs: &'a super::spatial::MobSnapshot,
    pub tags: &'a Arc<BTreeMap<String, super::MobTagValue>>,
    pub confined_region: Option<&'a super::confined::ConfinedRegion>,
    pub scripted: ScriptedReplies<'a>,
    pub rng: &'a mut MobRng,
}

#[derive(Default)]
pub struct ScriptedReplies<'a>(std::slice::IterMut<'a, Option<AiNodeDecision>>);

impl<'a> ScriptedReplies<'a> {
    pub fn new(replies: &'a mut [Option<AiNodeDecision>]) -> Self {
        ScriptedReplies(replies.iter_mut())
    }

    pub fn take_next(&mut self) -> Option<AiNodeDecision> {
        self.0.next().and_then(Option::take)
    }
}

impl AiCtx<'_> {
    pub fn path_params(&self) -> super::path::PathParams {
        super::path::PathParams::for_body(self.head, self.half_width).tolerating(self.tolerated)
    }

    pub fn entity_alive(&self, who: EntityRef) -> bool {
        match who {
            EntityRef::Player(pid) => self.players.iter().any(|a| a.id == pid),
            EntityRef::Mob(id) => self.mobs.live(id).is_some(),
        }
    }

    pub fn live_mob(&self, id: u64) -> Option<&AiMob> {
        self.mobs.live(id)
    }

    pub fn entity_pos(&self, who: EntityRef) -> Option<petramond_math::world_pos::WorldPos> {
        match who {
            EntityRef::Player(pid) => self.players.iter().find(|a| a.id == pid).map(|a| a.pos),
            EntityRef::Mob(id) => self
                .mobs
                .live(id)
                .map(|m| m.pos + Vec3::new(0.0, super::def(m.kind).size.height * 0.5, 0.0)),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AttackIntent {
    pub target: EntityRef,
    pub damage: f32,
    pub knockback: f32,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct BehaviorOutput {
    pub goal: Option<IVec3>,
    pub head_look: Option<HeadLook>,
    pub facing: Option<f32>,
    pub speed_scale: Option<f32>,
    pub idle_anim: Option<u8>,
    pub attack: Option<AttackIntent>,
    pub animation: Option<String>,
    pub target: Option<EntityRef>,
    pub claims: ChannelClaims,
    pub tag_writes: Vec<(String, Option<super::MobTagValue>)>,
}

macro_rules! for_each_channel {
    ($m:ident) => {
        $m!(goal => Goal);
        $m!(head_look => HeadLook);
        $m!(facing => Facing);
        $m!(speed_scale => SpeedScale);
        $m!(idle_anim => IdleAnim);
        $m!(attack => Attack);
        $m!(animation => Animation);
        $m!(target => Target);
    };
}

impl BehaviorOutput {
    pub fn filled(&self) -> ChannelClaims {
        let mut filled = ChannelClaims::NONE;
        macro_rules! mark {
            ($field:ident => $channel:ident) => {
                if self.$field.is_some() {
                    filled = filled.with(DecisionChannel::$channel);
                }
            };
        }
        for_each_channel!(mark);
        filled
    }

    fn settle(&mut self, offer: BehaviorOutput, settled: &mut ChannelClaims) {
        let closing = offer.claims | offer.filled();
        macro_rules! take {
            ($field:ident => $channel:ident) => {
                if !settled.contains(DecisionChannel::$channel) {
                    self.$field = offer.$field;
                }
            };
        }
        for_each_channel!(take);
        self.claims |= offer.claims;
        self.tag_writes.extend(offer.tag_writes);
        *settled |= closing;
    }
}

pub trait AiBehavior: Send {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput;

    fn scripted(&self) -> Option<&ScriptedNode> {
        None
    }
}

struct Entry {
    priority: u8,
    behavior: Box<dyn AiBehavior>,
}

#[derive(Default)]
pub struct Brain {
    entries: Vec<Entry>,
}

impl Brain {
    pub fn new() -> Self {
        Brain {
            entries: Vec::new(),
        }
    }

    pub fn scripted_nodes(&self) -> impl Iterator<Item = &ScriptedNode> + '_ {
        self.entries.iter().filter_map(|e| e.behavior.scripted())
    }

    pub fn has_scripted(&self) -> bool {
        self.scripted_nodes().next().is_some()
    }

    pub fn with_boxed(mut self, priority: u8, behavior: Box<dyn AiBehavior>) -> Self {
        self.entries.push(Entry { priority, behavior });
        self.entries.sort_by_key(|entry| Reverse(entry.priority));
        self
    }

    pub fn decide(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let mut decision = BehaviorOutput::default();
        let mut settled = ChannelClaims::NONE;
        for entry in &mut self.entries {
            let out = entry.behavior.tick(ctx);
            decision.settle(out, &mut settled);
        }
        decision
    }
}

#[cfg(test)]
mod tests;
