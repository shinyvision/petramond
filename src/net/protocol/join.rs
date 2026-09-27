use serde::{Deserialize, Serialize};

use crate::player::PlayerId;
use petramond_math::math::IVec3;
use petramond_world::crafting::CraftingRecipeData;

use super::{ItemSlotWire, Transform};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModEntry {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinCredential {
    Ticket(String),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinRejectReason {
    BadProof,
    InvalidName(String),
    AlreadyConnected,
    ServerFull,
    AccountRequired,
    AccountNotAccepted,
    AccountAlreadyOnline,
    AccountRejected(String),
    AccountUnavailable(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameTables {
    pub blocks: Vec<String>,
    pub biomes: Vec<String>,
    pub items: Vec<String>,
    pub mobs: Vec<String>,
    pub sounds: Vec<String>,
    pub effects: Vec<String>,
    pub emitters: Vec<String>,
    pub conditions: Vec<String>,
    pub animators: Vec<crate::player::animator::AnimatorNames>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfRestore {
    pub transform: Transform,
    pub mode: u8,
    pub health: i32,
    pub bed_spawn: Option<(IVec3, IVec3)>,
    pub effects: Vec<(String, u32)>,
    pub inventory: Vec<Option<ItemSlotWire>>,
    pub active_slot: u8,
    pub craft_craftable_only: bool,
    pub unlocked_recipes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinData {
    pub player_id: PlayerId,
    pub player_name: String,
    pub seed: u32,
    pub clock: u64,
    pub tables: NameTables,
    pub self_restore: SelfRestore,
    pub crafting_recipes: Vec<CraftingRecipeData>,
    pub players: Vec<(PlayerId, String)>,
    pub client_policy: ClientPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPolicy {
    pub presentation_packs: bool,
}

impl Default for ClientPolicy {
    fn default() -> Self {
        Self {
            presentation_packs: true,
        }
    }
}
