//! The acting player's body and rig: the calls BOTH sides serve — the server
//! authoritatively, a client instance as a prediction against its own mirror
//! (held and bone poses, displays, animator writes, the use gesture, the
//! carried inventory, and the dispatch's actor).
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    /// The acting player's body and rig: the calls BOTH sides serve — the server
    /// authoritatively, a client instance as a prediction against its own mirror
    /// (held and bone poses, displays, animator writes, the use gesture, the
    /// carried inventory, and the dispatch's actor).
    BodyCall {
        /// The player's current state. → [`HostRet::Player`](crate::HostRet::Player).
        PlayerState => legal(SERVER_CLIENT, Sim, Read),
        /// Claim a HELD-ITEM POSE on one body, per hand: an extra Blockbench
        /// display transform ([`HeldPose`](crate::HeldPose)) composed onto
        /// whatever that hand already holds, in first person and on every
        /// observer's third-person body.
        ///
        /// Authored in the `display` block's own units and composed OUTSIDE the
        /// item's hold, so the offset moves the item within the hold frame. One
        /// per view, because the two views start from different authored poses.
        /// The off hand needs no separate authoring — the engine mirrors by
        /// Blockbench's own left-hand rule — and every held render kind (bbmodel,
        /// sprite, block cube) wears it alike.
        ///
        /// `None` releases a hand; claims resolve last-wins in claimant order.
        /// TRANSIENT, and the client eases between updates so a 20 Hz publisher
        /// still glides. A non-finite component is a [`HostRet::Err`](crate::HostRet::Err), never a
        /// silently dropped pose.
        ///
        /// Legal on a CLIENT instance for the LOCAL player, which is how a pose
        /// presents on the frame the input asks for it rather than a round trip
        /// later. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        SetPlayerHeldPose {
            player: PlayerId,
            main: Option<crate::HeldPose>,
            off: Option<crate::HeldPose>,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Claim BONE OFFSETS on one body — rotate or shift named bones of the
        /// player rig, composed onto whatever animation already posed them.
        ///
        /// The body counterpart of
        /// [`SetPlayerHeldPose`](Self::SetPlayerHeldPose): that poses what a hand
        /// is HOLDING, this poses the hand. An offset on a shoulder carries
        /// through the whole arm and everything in its fist.
        ///
        /// Degrees about the bone's posed pivot, translations in 1/16-block pixels
        /// ([`BonePoseData`](crate::BonePoseData)); bones are named from
        /// [`bone`](crate::bone). Unlike a held pose, every claimant's offsets
        /// APPLY — a rotation about one joint composes with another about a
        /// different one. An empty list releases; TRANSIENT; a non-finite
        /// component is a [`HostRet::Err`](crate::HostRet::Err) and a name this rig lacks is dropped.
        ///
        /// Legal on a CLIENT instance for the LOCAL player, the same predicted
        /// path as [`SetPlayerHeldPose`](Self::SetPlayerHeldPose).
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        SetPlayerBonePose {
            player: PlayerId,
            bones: Vec<crate::BonePoseData>,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Take `player`'s current USE GESTURE — one press of the interact button —
        /// and keep it until they let go. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such
        /// reachable session).
        ///
        /// A gesture has at most one owner. Most interactions resolve inside it and
        /// leave it free, which is what lets a held button keep placing blocks or
        /// flapping a door; this is how something CONTINUOUS says otherwise. While
        /// it is held nothing else is offered the button — no repeat, no fresh
        /// click — and [`PlayerSnapshot::holds_use`](crate::PlayerSnapshot::holds_use) answers `true` for the owner
        /// and nobody else.
        ///
        /// Call it from a [`EventKind::UseUnclaimed`](crate::EventKind::UseUnclaimed) handler — the fall-through
        /// fired once the whole interact chain has passed. Taking the press is not
        /// an interaction: nothing happened to the world and no hand jabs, so
        /// whoever takes it poses the body itself. The engine's own eat holds a
        /// gesture the same way, which is why a held button does not eat a stack.
        ///
        /// Legal on a CLIENT instance for the LOCAL player, so a rule that runs on
        /// both sides presents on the frame the button goes down.
        ///
        /// [`PlayerSnapshot::holds_use`]: crate::PlayerSnapshot::holds_use
        /// [`EventKind::UseUnclaimed`]: crate::EventKind::UseUnclaimed
        HoldUse {
            player: PlayerId,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Claim what each of `player`'s hands DISPLAYS — an item (by registry
        /// NAME) whose art (sprite or model, both views, every observer) draws
        /// in place of the held stack's own, or `None` to release a hand.
        /// Presentation only: the inventory, the hotbar and every simulation
        /// read keep the real stack, so this is the seam for an item whose LOOK
        /// follows a rule of the mod's — a bow drawn through its pull frames, a
        /// torch that lights, a book that opens. Resolves like
        /// [`SetPlayerHeldPose`] (the LAST claim in mod-id order wins a hand;
        /// releasing uncovers another's), and a display changing under a hand
        /// never resets the hand's eased pose — the STACK did not change.
        /// TRANSIENT — re-state it every tick. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no
        /// such reachable session); an unknown item name is [`HostRet::Err`](crate::HostRet::Err).
        ///
        /// Legal on a CLIENT instance for the LOCAL player, the same predicted
        /// path as [`SetPlayerHeldPose`].
        ///
        /// [`SetPlayerHeldPose`]: Self::SetPlayerHeldPose
        SetPlayerHeldDisplay {
            player: PlayerId,
            main: Option<String>,
            off: Option<String>,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Every stack `player` CARRIES, in the one layout every inventory read
        /// and spend on this surface shares: the grid in slot order (the hotbar
        /// first, then the main grid), then the off hand as the LAST entry.
        /// `None` slots are empty. The read a rule makes BEFORE it commits to a
        /// gesture — a bow with no arrow to loose has no draw to show — and the
        /// snapshot any counting, sorting or variant-picking rule derives from.
        /// → [`HostRet::ContainerSlots`](crate::HostRet::ContainerSlots) (`None` for no such reachable session).
        ///
        /// Legal on a CLIENT instance for the LOCAL player, answered from the
        /// replicated inventory — so a rule that gates on it predicts the same
        /// answer the server reaches.
        PlayerInventory {
            player: PlayerId,
        } => legal(SERVER_CLIENT, Sim, Read),
        /// Set graph PARAMS on one body's rig animators — the animator's
        /// `set` primitive at the ABI. Each rig's graph declares its params; a
        /// mod's overlay of the animator document may add its own. The values
        /// stand until re-stated: TRANSIENT and keyed by claimant like every
        /// body claim — the list REPLACES this mod's previous params (an empty
        /// list releases them all), the last claimant in mod-id order wins a
        /// contested param, and a released param falls back to the engine's
        /// own value. A rig name no registered rig carries, or a param name
        /// the rig's graph lacks, is a [`HostRet::Err`](crate::HostRet::Err).
        ///
        /// Params feed every formula a graph has — its layer weights, rule
        /// conditions and GATES — so a mod stands one of the engine's gestures
        /// down on a hand it animates itself by setting the param the rig's gate
        /// for that gesture reads (the rig's animator document names its gates).
        ///
        /// Legal on a CLIENT instance for the LOCAL player, where it is its own
        /// predicted path: the same call from a client mod owns each named
        /// param locally from then on.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        SetPlayerAnimatorParams {
            player: PlayerId,
            params: Vec<crate::AnimatorParam>,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Hold montages in SLOTS of one body's rig animators — the animator's
        /// `play` primitive at the ABI ([`AnimatorPlay`]): a clip in a declared
        /// slot, on the caller's clock ([`AnimatorClock`](crate::AnimatorClock):
        /// scrubbed at its own progress or free-running at a rate). TRANSIENT
        /// and keyed by claimant: the list REPLACES this mod's previous plays
        /// (an empty list releases them all — the slot fades back to whatever
        /// the graph does), the last claimant in mod-id order wins a contested
        /// slot. A rig, slot or clip the engine lacks, or a non-finite progress
        /// or rate, is a [`HostRet::Err`](crate::HostRet::Err).
        ///
        /// Legal on a CLIENT instance for the LOCAL player, where it is its own
        /// predicted path: the same call from a client mod owns each named slot
        /// locally from then on.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        ///
        /// [`AnimatorPlay`]: crate::AnimatorPlay
        SetPlayerAnimatorPlays {
            player: PlayerId,
            plays: Vec<crate::AnimatorPlay>,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Fire a graph EVENT on one body's rig animator — the animator's `fire`
        /// primitive at the ABI: the graph's rules answer it on every mirror
        /// (a montage, a hit-stop, a stop) exactly as they answer the engine's
        /// own events. An edge, not a claim: nothing to release. A rig name no
        /// registered rig carries, or an event the rig's graph does not
        /// declare, is a [`HostRet::Err`](crate::HostRet::Err). Only an OBSERVED rig's events reach
        /// other players' mirrors; the player's own always hear it.
        ///
        /// Legal on a CLIENT instance for the LOCAL player; an event a client
        /// mod fired locally is not fired again when the server's echo arrives.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such reachable session).
        FirePlayerAnimatorEvent {
            player: PlayerId,
            rig: String,
            event: String,
        } => legal(SERVER_CLIENT, Sim, Write),
        /// Read one clip of a player rig — its length, loop and timeline markers
        /// — so a mod times what it lands to the clip's own `impact` rather than
        /// repeating the number. Legal on both sides.
        /// → [`HostRet::AnimationClip`](crate::HostRet::AnimationClip) (`None` = no such clip).
        AnimationClip {
            rig: String,
            clip: String,
        } => legal(SERVER_CLIENT, Any, Read),
        /// The session the running dispatch acts for, or `None` for an
        /// actor-less dispatch (see "Player addressing" on [`HostCall`](crate::HostCall)).
        /// → [`HostRet::ActingPlayer`](crate::HostRet::ActingPlayer).
        ActingPlayer => legal(SERVER_CLIENT, Sim, Read),
    }
}
