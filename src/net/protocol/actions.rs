use serde::{Deserialize, Serialize};

use crate::mathh::{IVec3, Vec3};

use super::Transform;

/// Per-connection monotonic id for discrete mutating intents that need an
/// [`ActionOutcome`]. Client allocates; server echoes.
pub(crate) type ClientRequestId = u32;

/// Coarse deny reasons for [`ActionOutcome`] — enough for rollback/UI.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ActionDenyReason {
    OutOfReach,
    InvalidSlot,
    Busy,
    Denied,
    TooFast,
    BadTool,
}

/// Server answer to one client request id (accept or deny).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ActionOutcome {
    pub id: ClientRequestId,
    pub accepted: bool,
    pub reason: Option<ActionDenyReason>,
}

/// Per-frame (local) / throttled (TCP) client transform + held intents.
///
/// Movement F2: `wishdir` / `jump` / `sprint` / `sneak` are the authoritative
/// input; the server integrates physics on the fixed tick. `pos`/`vel`/
/// `on_ground` remain the client's prediction (used for soft comparison / fall
/// bookkeeping until a hard correct ships via [`SelfTransform`]).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerUpdate {
    /// The client's predicted transform (see the struct doc).
    pub transform: Transform,
    pub on_ground: bool,
    /// Sneak held — movement intent (half speed + edge guard, integrated
    /// server-side like `wishdir`) AND the interact-vs-place gate; gameplay-
    /// gated client-side like the rest of the predicted input.
    pub sneak: bool,
    /// Gameplay input live (false while a screen owns focus — server forces
    /// held intents off, mirroring `capture_intent`).
    pub gameplay: bool,
    pub break_held: bool,
    pub use_held: bool,
    /// The client's raycast target (block + face normal), reach-validated
    /// server-side.
    pub target: Option<TargetRef>,
    pub hotbar_slot: u8,
    pub held_rotation: u8,
    /// Horizontal/3D wish direction (unit or zero) for server-side movement.
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TargetRef {
    pub block: IVec3,
    pub normal: IVec3,
}

/// How much a cursor throw takes off the held stack: the whole stack
/// (primary click outside the panel) or a single item (secondary click).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ThrowAmount {
    All,
    One,
}

/// One-shot player actions, applied in arrival order on the next server tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum PlayerAction {
    /// Secondary press: the interact/eat/use/place ladder. `mob` is the mob
    /// under the crosshair at click time (stable id) — the shear target, like
    /// `AttackClick`'s. `target` is the block under the crosshair AT CLICK
    /// TIME: the server resolves the interact/place against THIS cell, never
    /// a fresher look latch — otherwise a click racing the crosshair places
    /// somewhere the client's ghost isn't. The server still validates reach,
    /// and items declaring a water-stopping ray must match its authoritative
    /// first hit. The authoritative selected slot/item is captured with the
    /// click; a hotbar change before tick consumption denies the whole
    /// attempt instead of changing which item receives the target.
    /// `request_id` is set when the
    /// client opened a ledger entry (place ghost or track-only);
    /// presentation-only jabs may omit it. `predicted` says whether the
    /// client actually PRESENTED a full place (ghost + sound) — it gates the
    /// initiator's `BlockPlaced` echo strip only, exactly like
    /// `BreakFinished.predicted`: an unpredicted placement (oriented model,
    /// replace-in-place, slab stack, frozen ledger) must keep its event or
    /// the initiator never hears their own place.
    UseClick {
        mob: Option<u64>,
        target: Option<TargetRef>,
        request_id: Option<ClientRequestId>,
        predicted: bool,
        /// Whether the client played its own P0 hand jab for this click (its
        /// "predictably does something" verdict). When the server consumes a
        /// click the client could NOT foresee — a mod-cancelled item use or
        /// block interact — it echoes `SelfEvents::used_unpredicted` so the
        /// jab still plays exactly once.
        jabbed: bool,
    },
    /// Primary press: attack the mob under the crosshair (stable mob id), the
    /// remote PLAYER under the crosshair (`PlayerId` byte — PvP), or punch the
    /// air. The client sends AT MOST ONE of `mob`/`player` (targeting picks
    /// the nearest); the server validates the player target (alive,
    /// non-spectator, within reach) before any damage.
    AttackClick {
        mob: Option<u64>,
        player: Option<u8>,
    },
    Drop {
        all: bool,
        request_id: ClientRequestId,
    },
    /// Throw from the cursor-held GUI stack out into the world (click outside
    /// the panel while dragging).
    ThrowCursor {
        amount: ThrowAmount,
        request_id: ClientRequestId,
    },
    /// Client finished mining locally; server validates tool/reach and the
    /// duration against ITS OWN observed mining window (never client-reported
    /// time).
    BreakFinished {
        request_id: ClientRequestId,
        pos: IVec3,
        /// Wire item id of the tool used (`None` = bare hand).
        tool_item_id: Option<u8>,
        /// Whether the client applied the break optimistically (replica clear
        /// + local sound/burst). Gates the initiator's echo strip: a
        /// track-only finish (frozen ledger, replica disagreement) never
        /// presented, so its `BlockBroken` world event must still be
        /// delivered. Presentation-only — the validation path ignores it.
        predicted: bool,
    },
    Wake,
    Respawn,
    /// Request a survival/spectator toggle (Ctrl+Y). Applied at message time
    /// only when the sending session is an operator; the session's fall
    /// tracker re-anchors so the switch is never measured as a fall. The
    /// authoritative mode flows back via [`SelfState::mode`].
    ToggleMode,
    /// The inventory key (E): the server opens the inventory crafting session
    /// on the next tick and answers with an [`OpenScreen::Gui`] ack carrying
    /// the inventory kind (the client's screen is already up).
    OpenInventory,
    CloseMenu,
}

/// [`crate::controls::PointerButton`] on the wire.
pub(crate) fn button_to_wire(button: crate::controls::PointerButton) -> u8 {
    match button {
        crate::controls::PointerButton::Primary => 0,
        crate::controls::PointerButton::Secondary => 1,
    }
}

pub(crate) fn button_from_wire(button: u8) -> crate::controls::PointerButton {
    match button {
        0 => crate::controls::PointerButton::Primary,
        _ => crate::controls::PointerButton::Secondary,
    }
}
