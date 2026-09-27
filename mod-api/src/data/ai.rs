use serde::{Deserialize, Serialize};

use super::*;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AiNodeCtx {
    pub mob_id: u64,
    pub pos: [f64; 3],
    pub cell: [i32; 3],
    pub yaw: f32,
    pub tick: u64,
    pub player_id: PlayerId,
    pub player_pos: [f64; 3],
    pub nav_idle: bool,
    pub in_fluid: Option<BlockId>,
    pub target: Option<EntityRef>,
    pub attacker: Option<(EntityRef, u32)>,
    pub player_held: Option<ItemId>,
    pub player_foothold: Option<[i32; 3]>,
    pub tags: Vec<(String, MobTagValue)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobTagWrite {
    pub key: String,
    pub value: Option<MobTagValue>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DecisionChannel {
    Goal = 0,
    HeadLook = 1,
    Facing = 2,
    SpeedScale = 3,
    IdleAnim = 4,
    Attack = 5,
    Animation = 6,
    Target = 7,
}

impl DecisionChannel {
    pub const ALL: [DecisionChannel; 8] = [
        DecisionChannel::Goal,
        DecisionChannel::HeadLook,
        DecisionChannel::Facing,
        DecisionChannel::SpeedScale,
        DecisionChannel::IdleAnim,
        DecisionChannel::Attack,
        DecisionChannel::Animation,
        DecisionChannel::Target,
    ];

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct ChannelClaims(u8);

impl ChannelClaims {
    pub const NONE: ChannelClaims = ChannelClaims(0);
    pub const ALL: ChannelClaims = ChannelClaims::of(&DecisionChannel::ALL);

    pub const fn of(channels: &[DecisionChannel]) -> ChannelClaims {
        let mut bits = 0;
        let mut i = 0;
        while i < channels.len() {
            bits |= channels[i].bit();
            i += 1;
        }
        ChannelClaims(bits)
    }

    pub const fn contains(self, channel: DecisionChannel) -> bool {
        self.0 & channel.bit() != 0
    }

    pub const fn with(self, channel: DecisionChannel) -> ChannelClaims {
        ChannelClaims(self.0 | channel.bit())
    }

    pub const fn union(self, other: ChannelClaims) -> ChannelClaims {
        ChannelClaims(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for ChannelClaims {
    type Output = ChannelClaims;
    fn bitor(self, rhs: ChannelClaims) -> ChannelClaims {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for ChannelClaims {
    fn bitor_assign(&mut self, rhs: ChannelClaims) {
        *self = self.union(rhs);
    }
}

impl std::fmt::Debug for ChannelClaims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set()
            .entries(DecisionChannel::ALL.iter().filter(|&&c| self.contains(c)))
            .finish()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AiNodeDecision {
    pub goal: Option<[i32; 3]>,
    pub head_look: Option<[f32; 2]>,
    pub facing: Option<f32>,
    pub speed_scale: Option<f32>,
    pub idle_anim: Option<u8>,
    pub attack: Option<[f32; 2]>,
    pub animation: Option<String>,
    pub target: Option<EntityRef>,
    pub claims: ChannelClaims,
    pub tags: Vec<MobTagWrite>,
}
