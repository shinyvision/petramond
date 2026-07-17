use serde::{Deserialize, Serialize};

use crate::crafting::CraftingRecipeData;
use crate::mathh::IVec3;
use crate::server::player::PlayerId;

use super::{ItemSlotWire, Transform};

/// One enabled mod, as the handshake reports it. Version is display-only —
/// compatibility checks are by ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModEntry {
    pub id: String,
    pub version: String,
}

/// Why a `Join` was refused. Kept for protocol stability (clients still handle
/// a reject) — the server no longer emits `NameTaken`: a duplicate name is
/// auto-deduped with a numeric suffix at admission instead (see
/// `ServerGame::admit_remote_player`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum JoinRejectReason {
    /// A connected player already uses this name (case-insensitive).
    NameTaken,
}

/// The server's registry name tables, in server-runtime-id order — the wire's
/// id vocabulary (the on-disk analogue is `palette.json`). The client builds
/// server-id→client-id LUTs from these at join.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NameTables {
    pub blocks: Vec<String>,
    pub items: Vec<String>,
    pub mobs: Vec<String>,
    pub sounds: Vec<String>,
    pub effects: Vec<String>,
    /// `particle_emitters.json` bundle keys, in server-id order.
    pub emitters: Vec<String>,
}

/// The joining player's own restored state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelfRestore {
    pub transform: Transform,
    /// `PlayerMode` as its discriminant (tiny closed enum).
    pub mode: u8,
    pub health: i32,
    pub bed_spawn: Option<(IVec3, IVec3)>,
    /// Active status effects by registry NAME + remaining ticks (names, not
    /// ids: effects are the one registry small enough that names are cheap and
    /// they already persist by name in level.dat).
    pub effects: Vec<(String, u32)>,
    /// All inventory slots in index order, then the cursor stack last.
    pub inventory: Vec<Option<ItemSlotWire>>,
    /// The active hotbar slot, so the restored selection survives the join.
    pub active_slot: u8,
    /// The recipe browser's craftable-only filter preference.
    pub craft_craftable_only: bool,
}

/// Everything a client needs to enter the world, sent on `JoinAccept`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct JoinData {
    pub player_id: PlayerId,
    pub seed: u32,
    /// The day/night clock (`petramond:clock` contract) so the sky renders right
    /// from the first frame.
    pub clock: u64,
    pub tables: NameTables,
    pub self_restore: SelfRestore,
    /// The enabled player-crafting catalog, sent once and resolved locally by
    /// registry name. Unlike live slot contents, these rows carry no numeric
    /// registry ids and therefore need no transport remap.
    pub crafting_recipes: Vec<CraftingRecipeData>,
    /// Already-connected players (id, name), local player excluded.
    pub players: Vec<(PlayerId, String)>,
}
