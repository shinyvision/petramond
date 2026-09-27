use serde::{Deserialize, Serialize};

use petramond_math::math::{IVec3, Vec3};

use super::Transform;

pub type ClientRequestId = u32;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionDenyReason {
    OutOfReach,
    InvalidSlot,
    Busy,
    Denied,
    TooFast,
    BadTool,
}

impl ActionOutcome {
    pub fn deny(id: ClientRequestId, reason: ActionDenyReason) -> ActionOutcome {
        ActionOutcome {
            id,
            accepted: false,
            reason: Some(reason),
        }
    }

    #[allow(dead_code)]
    pub fn accept(id: ClientRequestId) -> ActionOutcome {
        ActionOutcome {
            id,
            accepted: true,
            reason: None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub id: ClientRequestId,
    pub accepted: bool,
    pub reason: Option<ActionDenyReason>,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerUpdate {
    pub transform: Transform,
    pub on_ground: bool,
    pub sneak: bool,
    pub gameplay: bool,
    pub break_held: bool,
    pub use_held: bool,
    pub target: Option<TargetRef>,
    pub hotbar_slot: u8,
    pub held_rotation: u8,
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRef {
    pub block: IVec3,
    pub normal: IVec3,
    pub spot: [u8; 3],
}

impl TargetRef {
    pub fn of_hit(hit: &crate::player::RaycastHit) -> Self {
        Self {
            block: hit.block,
            normal: hit.normal,
            spot: std::array::from_fn(|a| (hit.spot[a].clamp(0.0, 1.0) * 255.0).round() as u8),
        }
    }

    pub fn face(block: IVec3, normal: IVec3) -> Self {
        Self {
            block,
            normal,
            spot: [127; 3],
        }
    }

    pub fn spot_fraction(&self) -> [f32; 3] {
        std::array::from_fn(|a| f32::from(self.spot[a]) / 255.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThrowAmount {
    All,
    One,
}

/// One-shot actions, applied in arrival order on the next server tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlayerAction {
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
        jabbed: bool,
    },
    AttackClick {
        mob: Option<u64>,
        player: Option<u8>,
    },
    Drop {
        all: bool,
        request_id: ClientRequestId,
    },
    ThrowCursor {
        amount: ThrowAmount,
        request_id: ClientRequestId,
    },
    BreakFinished {
        request_id: ClientRequestId,
        pos: IVec3,
        tool_item_id: Option<u16>,
        predicted: bool,
    },
    Wake,
    Respawn,
    ToggleMode,
    ToggleCreative,
    ToggleFlight,
    Creative(crate::schematic::CreativeAction),
    Schematic(crate::schematic::share::SchematicRequest),
    OpenInventory,
    CloseMenu,
}

pub fn button_to_wire(button: petramond_world::gui_state::PointerButton) -> u8 {
    match button {
        petramond_world::gui_state::PointerButton::Primary => 0,
        petramond_world::gui_state::PointerButton::Secondary => 1,
    }
}

pub fn button_from_wire(button: u8) -> petramond_world::gui_state::PointerButton {
    match button {
        0 => petramond_world::gui_state::PointerButton::Primary,
        _ => petramond_world::gui_state::PointerButton::Secondary,
    }
}
