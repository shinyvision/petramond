use serde::{Deserialize, Serialize};

use crate::player::PlayerId;
use petramond_math::math::Vec3;
use petramond_world::chunk::{ChunkPos, SectionPos};

mod actions;
mod chat;
mod entities;
mod join;
mod menu;
mod state;
mod terrain;
mod tick;

#[cfg(test)]
pub(crate) mod tests;

pub use actions::*;
pub use chat::*;
pub use entities::*;
pub use join::*;
pub use menu::*;
pub use state::*;
pub use terrain::*;
pub use tick::*;

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub pos: petramond_math::world_pos::WorldPos,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSlotWire {
    pub item_id: u16,
    pub count: u8,
    pub data: Option<Vec<u8>>,
}

impl ItemSlotWire {
    pub fn from_stack(st: petramond_world::item::ItemStack) -> Self {
        ItemSlotWire {
            item_id: st.item.0,
            count: st.count,
            data: petramond_world::item::variant::blob(st.variant).map(|b| (*b).clone()),
        }
    }

    pub fn to_stack(&self) -> petramond_world::item::ItemStack {
        let mut st = petramond_world::item::ItemStack::new(
            petramond_world::item::ItemType(self.item_id),
            self.count,
        );
        if let Some(blob) = &self.data {
            match petramond_world::item::variant::intern_blob(blob) {
                Ok(v) => st.variant = v,
                Err(e) => log::warn!("wire slot: instance-data blob dropped: {e}"),
            }
        }
        st
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientToServer {
    Hello {
        protocol: u16,
    },
    ModQuery,
    Join {
        credential: JoinCredential,
        key: crate::net::identity::PlayerKey,
        proof: Vec<u8>,
        view_distance: u8,
        cached_sections: Vec<SectionCacheClaim>,
    },
    PlayerUpdate(PlayerUpdate),
    Action(PlayerAction),
    CreativeCursor {
        item: Option<String>,
        request_id: ClientRequestId,
    },
    MenuClick {
        slot: MenuSlotWire,
        button: u8,
        shift: bool,
        gather: bool,
        request_id: ClientRequestId,
    },
    MenuDrag {
        slots: Vec<MenuSlotWire>,
        button: u8,
        request_id: ClientRequestId,
    },
    MenuDrop {
        slot: MenuSlotWire,
        all: bool,
        request_id: ClientRequestId,
    },
    MenuSwapOffHand {
        slot: MenuSlotWire,
        request_id: ClientRequestId,
    },
    CraftRecipe {
        recipe: String,
        bulk: bool,
        request_id: ClientRequestId,
    },
    ChatSend {
        text: String,
    },
    StreamBatchAck {
        messages_per_second: f32,
    },
    SectionCacheMiss {
        pos: SectionPos,
    },
    SetViewDistance {
        chunks: u8,
    },
    SetCraftFilter {
        craftable_only: bool,
    },
    Pause(bool),
    KeepAlive,
    Disconnect,
    /// The client's half of the key exchange, sent in the clear right after `HelloAck`. Every
    /// frame after it, in both directions, is sealed (`net::secure`).
    KeyExchange {
        key_share: crate::net::secure::KeyShare,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerToClient {
    HelloAck {
        protocol: u16,
        challenge: crate::net::identity::JoinChallenge,
        requires_account: bool,
        server_id: String,
        key_share: crate::net::secure::KeyShare,
    },
    HelloReject {
        server_protocol: u16,
    },
    ModList {
        mods: Vec<ModEntry>,
    },
    JoinAccept(Box<JoinData>),
    JoinReject {
        reason: JoinRejectReason,
    },
    ColumnData(ColumnPayload),
    SectionData(Box<SectionPayload>),
    LightData(LightPayload),
    SectionUnload {
        pos: SectionPos,
        cache_hash: Option<u64>,
    },
    ColumnUnload {
        pos: ChunkPos,
        cache_hashes: Vec<(i32, u64)>,
    },
    SectionCached {
        pos: SectionPos,
        hash: u64,
    },
    StreamBatchStart,
    StreamBatchEnd {
        count: u32,
    },
    Tick(Box<TickUpdate>),
    PlayerJoined {
        id: PlayerId,
        name: String,
    },
    PlayerLeft {
        id: PlayerId,
    },
    ChatLine(ChatLine),
    RecipesUnlocked {
        recipes: Vec<String>,
    },
    ModsDisabled {
        mods: Vec<String>,
    },
    ServerClosing,
    KeepAlive,
    Disconnect {
        reason: String,
    },
}
