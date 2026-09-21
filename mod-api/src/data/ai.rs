//! What an AI node is handed each dispatch and what it answers with.

use serde::{Deserialize, Serialize};

use super::*;

/// The read-only mob snapshot an [`GuestCall::AiNode`] decision sees.
///
/// The baseline fields (the mob's own state, the current tick, and the
/// nearest player's id/position) are always present. Fact fields beyond the
/// baseline are DECLARED INPUTS: the brain node row lists the facts its node
/// reads (`"inputs": ["player_held"]` in `mobs.json`), and only declared
/// facts are computed and shipped — an undeclared fact always reads `None`.
/// Every `player_*` fact describes the SAME player, [`player_id`]
/// (the nearest one), mutually consistent within a dispatch.
///
/// [`player_id`]: AiNodeCtx::player_id
/// [`GuestCall::AiNode`]: crate::GuestCall::AiNode
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AiNodeCtx {
    /// Stable id of the deciding mob — key per-mob guest state off it.
    pub mob_id: u64,
    /// Mob feet position (world space).
    pub pos: [f64; 3],
    /// Mob foothold voxel.
    pub cell: [i32; 3],
    /// Body facing (radians).
    pub yaw: f32,
    /// The current game tick — the same value `current_tick()` returns
    /// (dispatch runs once per owning mob per game tick), carried here so
    /// timekeeping costs no host call.
    pub tick: u64,
    /// Session id of the NEAREST player — the player every `player_*` fact
    /// in this snapshot describes.
    pub player_id: PlayerId,
    /// That player's body-centre (world space).
    pub player_pos: [f64; 3],
    /// True when the navigator has no active path ("the mob is idle").
    pub nav_idle: bool,
    /// The fluid the mob's body is in or resting on, if any.
    pub in_fluid: Option<BlockId>,
    /// The entity the WHOLE brain locked last tick (the settled
    /// [`AiNodeDecision::target`] across every node) — what an attack
    /// decision strikes when it names no target of its own.
    pub target: Option<EntityRef>,
    /// Who last damaged this mob, and how many ticks ago — the input every
    /// damage response (flee, inspect, retaliate) reads. Recorded by the
    /// damage pipeline; `None` until the mob is first hit.
    pub attacker: Option<(EntityRef, u32)>,
    /// DECLARED INPUT `"player_held"`: the nearest player's selected (held)
    /// item — resolve names via `ResolveItem` and compare (a lure, a beg, a
    /// trade gate all read this same fact). `None` when the input is
    /// undeclared, the hand is empty, or the player is a spectator.
    pub player_held: Option<ItemId>,
    /// DECLARED INPUT `"player_foothold"`: the mob-standable navigation
    /// foothold nearest that player (what the engine's `chase_player` paths
    /// toward) — the ready-made `goal` for any follow/approach node. `None`
    /// when the input is undeclared, the player is airborne or has no
    /// reachable foothold, or the player is more than 32 blocks away (the
    /// outer edge of player-reactive mob AI — the scan is skipped past it).
    pub player_foothold: Option<[i32; 3]>,
    /// The deciding mob's OWN tag map (baseline — the mob's own state),
    /// sorted by key: the same view `mob_tags_get` returns, without a host
    /// call. Persist per-mob node state by WRITING tags back through
    /// [`AiNodeDecision::tags`] instead of keying a guest-side map off
    /// `mob_id` — tag state lives, saves, and dies with the mob.
    pub tags: Vec<(String, MobTagValue)>,
}

/// One tag write a scripted node's decision carries back — applied by the
/// ENGINE after the detached dispatch returns (a node cannot call `mob_tag_set`
/// mid-decision). `value: None` deletes the key.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobTagWrite {
    /// Namespaced tag key. Must carry the deciding node's OWN `mod_id:`
    /// prefix — a decision may not write engine or foreign tags (unlike
    /// `mob_tag_set`, which may write exposed `petramond:*` keys).
    pub key: String,
    /// The value to store, or `None` to delete the key.
    pub value: Option<MobTagValue>,
}

/// One arbitrated channel of a mob's decision — the fields a brain node can
/// FILL, and can HOLD (see [`ChannelClaims`]). Engine and scripted nodes
/// speak the same set; the brain settles each channel independently.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DecisionChannel {
    /// The navigation destination.
    Goal = 0,
    /// The head orientation relative to the body.
    HeadLook = 1,
    /// The body facing.
    Facing = 2,
    /// The locomotion / gait rate multiplier.
    SpeedScale = 3,
    /// The `idle_*` clip to play.
    IdleAnim = 4,
    /// The melee strike to land.
    Attack = 5,
    /// The named clip to start.
    Animation = 6,
    /// The entity the mob is engaged on.
    Target = 7,
}

impl DecisionChannel {
    /// Every channel, in declaration order.
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

/// The set of [`DecisionChannel`]s a node HOLDS against every lower-priority
/// node this tick, whether or not it filled them. Filling a channel already
/// settles it (the highest-priority value wins); a hold settles it EMPTY —
/// how a fleeing or reeling mob stops a lower combat node from striking, or a
/// lower wander node from handing it a destination, while leaving the
/// channels it does not name (a head-look, an idle clip) free to compose in.
/// Tag writes are never arbitrated and cannot be held.
#[derive(Serialize, Deserialize, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct ChannelClaims(u8);

impl ChannelClaims {
    /// Holds nothing — the default for a node that only fills channels.
    pub const NONE: ChannelClaims = ChannelClaims(0);
    /// Holds every channel.
    pub const ALL: ChannelClaims = ChannelClaims::of(&DecisionChannel::ALL);

    /// The set holding exactly `channels`.
    pub const fn of(channels: &[DecisionChannel]) -> ChannelClaims {
        let mut bits = 0;
        let mut i = 0;
        while i < channels.len() {
            bits |= channels[i].bit();
            i += 1;
        }
        ChannelClaims(bits)
    }

    /// Whether `channel` is held.
    pub const fn contains(self, channel: DecisionChannel) -> bool {
        self.0 & channel.bit() != 0
    }

    /// This set plus `channel`.
    pub const fn with(self, channel: DecisionChannel) -> ChannelClaims {
        ChannelClaims(self.0 | channel.bit())
    }

    /// This set plus every channel of `other`.
    pub const fn union(self, other: ChannelClaims) -> ChannelClaims {
        ChannelClaims(self.0 | other.0)
    }

    /// Whether nothing is held.
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

/// One scripted node's contribution to a mob's tick — the same channels an
/// engine node fills (see [`DecisionChannel`]). The opinion fields default to
/// "no opinion"; the engine settles each channel with the highest-priority
/// node that filled or held it, across scripted and engine nodes alike.
/// `tags` is NOT arbitrated: every node's writes apply, in brain order.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AiNodeDecision {
    /// A navigation destination (world voxel) to path toward.
    pub goal: Option<[i32; 3]>,
    /// A desired head orientation `[yaw, pitch]` relative to the body.
    pub head_look: Option<[f32; 2]>,
    /// A desired body facing (world radians), eased at the species' turn
    /// rate. Non-finite values are ignored.
    pub facing: Option<f32>,
    /// A horizontal locomotion AND gait-rate multiplier for this tick,
    /// clamped to `0..=`[`MAX_MOB_SPEED_SCALE`](crate::MAX_MOB_SPEED_SCALE).
    /// Non-finite values are ignored.
    pub speed_scale: Option<f32>,
    /// An `idle_*` animation index to play.
    pub idle_anim: Option<u8>,
    /// A melee strike `[damage, knockback]` to land THIS tick on
    /// [`target`](Self::target) — or, when the decision names none, on the
    /// entity the whole brain locked last tick. No target, no strike:
    /// exactly the engine `melee_attack` rule.
    pub attack: Option<[f32; 2]>,
    /// A named model clip to START (phase 0) this tick, e.g. a wind-up.
    /// Nonempty, at most [`MAX_MOB_ANIM_NAME_BYTES`] bytes; anything else
    /// is dropped with a warning. Clips the model lacks are skipped.
    pub animation: Option<String>,
    /// The entity this node is engaged on. The settled value is latched by
    /// the engine and fed back as next tick's lock, so an attack node strikes
    /// what a perception node found.
    pub target: Option<EntityRef>,
    /// Channels this node holds EMPTY against lower-priority nodes.
    pub claims: ChannelClaims,
    /// Tag writes on the deciding mob itself, applied by the engine after the
    /// dispatch (own-namespace keys only; the 32-tag cap refuses NEW keys
    /// past it). This is the persistence channel for per-mob node state —
    /// see [`AiNodeCtx::tags`] for the read side.
    pub tags: Vec<MobTagWrite>,
}
