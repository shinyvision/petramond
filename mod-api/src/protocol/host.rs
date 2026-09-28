//! Guest → host requests and their replies.
//!
//! [`HostCall`] is nested by DOMAIN: one arm per family of calls
//! ([`BlockCall`], [`EntityCall`], [`KvCall`], ...), each family its own
//! enum in its own file, and each call declared together with its
//! [`Legality`] — the instance sides it is legal on, whether it is confined
//! to `mod_init`, and whether it mutates. The host routes on the outer arm
//! and matches each family exhaustively in its own handler, so adding a call
//! touches one domain file and one handler, and its legality cannot be
//! declared anywhere but next to it.
//!
//! On the wire a call is `[domain index][call index][fields]` (both indices
//! postcard varints): append a domain at the end of [`HostCall`], a call at
//! the end of its domain.

use serde::{Deserialize, Serialize};

pub use super::guest::GuestCall;
use crate::client::{ClientContext, ClientEntityData, ClientSurfaceColumn, ClientViewStateData};
use crate::data::{
    BlockInfoData, CollisionShape, EffectStateData, GuiValue, GuiViewerData, ItemEntityData,
    ItemInfoData, ItemStackData, LightData, MobAnimStateData, MobRidersData, MobSnapshot,
    MobTagLookup, MobTagValue, PlayerInputData, PlayerListEntry, PlayerSnapshot, RaycastHitData,
    RuntimeSide,
};
use crate::error::{ErrorCode, HostError};
use crate::ids::{BlockId, ItemId, MobId, PlayerId};
use crate::legality::{CallInfo, Legality};

macro_rules! host_domain {
    (
        $(#[$meta:meta])*
        $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident $({ $($field:tt)* })? => $legal:expr,
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(::serde::Serialize, ::serde::Deserialize, Clone, Debug, PartialEq)]
        pub enum $name {
            $(
                $(#[$vmeta])*
                $variant $({ $($field)* })?,
            )*
        }

        impl $name {
            pub const CALLS: &'static [$crate::CallInfo] = &[
                $($crate::CallInfo {
                    name: stringify!($variant),
                    legality: $legal,
                },)*
            ];

            pub const fn name(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => stringify!($variant),)*
                }
            }

            pub const fn legality(&self) -> $crate::Legality {
                match self {
                    $(Self::$variant { .. } => $legal,)*
                }
            }
        }
    };
}

mod actors;
mod blocks;
mod body;
mod client;
mod client_capture;
mod client_files;
mod client_media;
mod client_presentation;
mod conditions;
mod construction;
mod containers;
mod core;
mod entities;
mod gui;
mod item_motion;
mod kv;
mod memo;
mod player;
mod registry;
mod schematics;
mod sounds;
mod tags;
mod worldgen;

pub use self::core::CoreCall;
pub use actors::ActorCall;
pub use blocks::BlockCall;
pub use body::BodyCall;
pub use client::ClientCall;
pub use client_capture::ClientCaptureCall;
pub use client_files::ClientFileCall;
pub use client_media::ClientMediaCall;
pub use client_presentation::ClientPresentationCall;
pub use conditions::ConditionCall;
pub use construction::ConstructionCall;
pub use containers::ContainerCall;
pub use entities::EntityCall;
pub use gui::GuiCall;
pub use item_motion::ItemMotionCall;
pub use kv::KvCall;
pub use memo::MemoCall;
pub use player::PlayerCall;
pub use registry::RegistryCall;
pub use schematics::SchematicCall;
pub use sounds::SoundCall;
pub use tags::TagCall;
pub use worldgen::WorldgenCall;

macro_rules! host_calls {
    (
        $(#[$meta:meta])*
        pub enum HostCall {
            $(
                $(#[$vmeta:meta])*
                $wrap:ident($domain:ident),
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
        pub enum HostCall {
            $(
                $(#[$vmeta])*
                $wrap($domain),
            )*
        }

        impl HostCall {
            pub const DOMAINS: &'static [(&'static str, &'static [CallInfo])] =
                &[$((stringify!($wrap), $domain::CALLS),)*];

            pub const fn name(&self) -> &'static str {
                match self {
                    $(Self::$wrap(call) => call.name(),)*
                }
            }

            pub const fn legality(&self) -> Legality {
                match self {
                    $(Self::$wrap(call) => call.legality(),)*
                }
            }
        }

        $(
            impl From<$domain> for HostCall {
                fn from(call: $domain) -> Self {
                    Self::$wrap(call)
                }
            }
        )*

        pub mod calls {
            $(pub use super::$domain::*;)*
        }
    };
}

host_calls! {
    /// Guest → host: what a mod asks the engine for through `host_dispatch`,
    /// one arm per call domain. The host routes on the arm and each domain's
    /// handler matches its own enum exhaustively; every call's legality (sides,
    /// scope, access) is declared with it and read through
    /// [`HostCall::legality`].
    ///
    /// The world-touching calls are sim-scoped ([`Scope::Sim`](crate::Scope::Sim)):
    /// legal wherever a `SimCtx` is published (`mod_init`, tick systems, event
    /// handlers), refused with [`ErrorCode::NoContext`] outside any guest
    /// dispatch.
    ///
    /// # Item identity
    ///
    /// Items have ONE mod-facing identity: the registry NAME (`"petramond:coal"`,
    /// `"farming:wheat"` — the `item` field of an `items.json` row). Every
    /// name-addressed call speaks it, and [`ItemStackData`] carries it. The
    /// numeric [`ItemId`] is a session-scoped compact form for id-bearing
    /// payloads (events, [`PlayerCall::ConsumeHeld`]); bridge the two with
    /// [`RegistryCall::ResolveItem`] (name → id) and [`RegistryCall::ItemNames`]
    /// (id → name), and never persist numeric ids. The `key` field on
    /// `items.json` rows is engine-internal recipe plumbing and does not cross
    /// the ABI.
    ///
    /// # Mob addressing
    ///
    /// A LIVE mob has ONE address: its stable session id
    /// ([`MobSnapshot::id`], the `mob_id` field on every mob call and event
    /// payload). It survives unrelated removals and is the key for cross-tick
    /// mod state; the list `index` on [`MobSnapshot`] is only an intra-tick
    /// join key between snapshots and is accepted by no call. Dead
    /// (ragdolling) mobs are GONE to this surface — id-addressed reads answer
    /// `None`, writes answer `false`, exactly the live set
    /// [`EntityCall::MobsInRadius`] enumerates. Mob SPECIES are keyed by their
    /// `mobs.json` `key` string (`"petramond:sheep"`); the numeric [`MobId`]
    /// is its session-scoped compact form in payloads — bridge with
    /// [`RegistryCall::ResolveMob`] (key → id) and [`RegistryCall::MobNames`]
    /// (id → key), and never persist either the numeric species id or a live
    /// mob's session id.
    ///
    /// # Player addressing
    ///
    /// Every call or payload field that names a player carries the
    /// [`PlayerId`] newtype EXPLICITLY ([`PlayerCall::PlayerInput`],
    /// [`EntityCall::MobMount`], [`PlayerCall::ChatSend`] targets, event payloads
    /// like `InteractAttempt`/`PlayerDismounted`). This is the frozen rule for NEW
    /// surface: a player-touching call takes a `player_id` — never a bare `u8`,
    /// and never a new implicit-player call. [`PlayerCall::GiveItemTo`] (a give)
    /// and [`PlayerCall::SetPlayerHeldData`] (a write onto that player's held
    /// stack) are the reference examples: each names the session it acts on
    /// rather than inheriting one, and answers `false` when no such session is
    /// connected.
    ///
    /// Every dispatch has an ACTOR or none ([`BodyCall::ActingPlayer`]): an
    /// event handler acts for the event's player (the clicking, eating, damaged
    /// or dying one — a `player_died` handler acts for whoever died), while tick
    /// systems, block hooks, spawn picks, `mod_init` and a mob's actions are
    /// actor-less. The older single-player-era calls ([`BodyCall::PlayerState`],
    /// [`PlayerCall::GiveItem`], [`PlayerCall::Teleport`], [`GuiCall::GuiOpen`],
    /// ...) address the actor, and answer [`HostRet::Err`] in an actor-less
    /// dispatch — there is no privileged "host" player to fall back on. Each has
    /// an explicit twin appended in ABI 1.1 ([`PlayerCall::PlayerStateOf`],
    /// [`PlayerCall::TeleportPlayer`], [`GuiCall::GuiOpenFor`], ...; gated by
    /// [`Capabilities::EXPLICIT_PLAYERS`](crate::Capabilities::EXPLICIT_PLAYERS)),
    /// and enumerating sessions is explicit via [`PlayerCall::Players`].
    ///
    /// # Batch bounds
    ///
    /// Batched sim/registry calls (`GetBlocks`, `SetBlocks`, `ContainerGetMany`,
    /// `ContainerSet` slots, the `*Names` reverse resolvers, `ChatSend` targets)
    /// are capped at [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) entries per call.
    /// Exceeding the cap is refused with [`ErrorCode::LimitExceeded`] — the
    /// watchdog deliberately does not charge host-side work, so the cap is what
    /// keeps one call from stalling the tick; a mod splits at it. Client-instance
    /// calls carry their own (tighter, per-frame) documented caps.
    pub enum HostCall {
        Core(CoreCall),
        Block(BlockCall),
        Entity(EntityCall),
        Player(PlayerCall),
        Body(BodyCall),
        Sound(SoundCall),
        Kv(KvCall),
        Tag(TagCall),
        Registry(RegistryCall),
        Worldgen(WorldgenCall),
        Memo(MemoCall),
        Gui(GuiCall),
        Container(ContainerCall),
        Client(ClientCall),
        ItemMotion(ItemMotionCall),
        Condition(ConditionCall),
        Construction(ConstructionCall),
        Actor(ActorCall),
        Schematic(SchematicCall),
        ClientFile(ClientFileCall),
        ClientCapture(ClientCaptureCall),
        ClientPresentation(ClientPresentationCall),
        ClientMedia(ClientMediaCall),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum MemoClaim {
    Value(#[serde(with = "serde_bytes")] Vec<u8>),
    Lease,
    Pending,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum HostRet {
    Unit,
    U64(u64),
    Err(HostError),
    Bool(bool),
    Block(Option<BlockId>),
    Blocks(Vec<Option<BlockId>>),
    Light(Option<LightData>),
    Mobs(Vec<MobSnapshot>),
    Player(Box<PlayerSnapshot>),
    Bytes(#[serde(with = "serde_bytes")] Option<Vec<u8>>),
    MobTag(MobTagLookup),
    GuiValue(Option<GuiValue>),
    ContainerSlots(Option<Vec<Option<ItemStackData>>>),
    ItemInfo(Option<Box<ItemInfoData>>),
    ItemStack(Option<ItemStackData>),
    Effects(Vec<EffectStateData>),
    Containers(Vec<Option<Vec<Option<ItemStackData>>>>),
    RuntimeSide(RuntimeSide),
    ClientSurfaceColumns(Vec<Option<ClientSurfaceColumn>>),
    ClientTextSize([u16; 2]),
    ClientStorageValues(Vec<Option<serde_bytes::ByteBuf>>),
    Item(Option<ItemId>),
    ClientStorageRead(Option<Vec<Option<serde_bytes::ByteBuf>>>),
    Riders(Option<MobRidersData>),
    ModelGroup(Option<crate::ModelGroupData>),
    PlayerInput(Option<PlayerInputData>),
    MobAnimState(Option<MobAnimStateData>),
    MaybeByte(Option<u8>),
    MaybeI32(Option<i32>),
    Players(Vec<PlayerListEntry>),
    EnvParams(Vec<Option<[f32; 4]>>),
    BlockList(Vec<BlockId>),
    ItemList(Vec<ItemId>),
    Names(Vec<Option<String>>),
    MobKind(Option<MobId>),
    CollisionShape(Option<CollisionShape>),
    MobTags(Option<Vec<(String, MobTagValue)>>),
    SpawnedMob(Option<u64>),
    FoundBlocks(Option<Vec<[i32; 3]>>),
    Mob(Option<MobSnapshot>),
    ItemEntity(Option<Box<ItemEntityData>>),
    BytesMany(Vec<Option<Vec<u8>>>),
    ItemDataRows(Vec<(ItemId, String)>),
    BlockDataRows(Vec<(BlockId, String)>),
    UndergroundBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    TerrainSolid(Vec<bool>),
    SurfaceBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    Points(Option<Vec<[f64; 3]>>),
    Bools(Vec<bool>),
    GuiViewers(Vec<GuiViewerData>),
    BlockInfo(Option<Box<BlockInfoData>>),
    HeldStack(Option<ItemStackData>),
    Raycast(Option<RaycastHitData>),
    ItemEntities(Vec<ItemEntityData>),
    StructureInfo(Option<Box<crate::StructureInfoData>>),
    Loot(Option<Vec<ItemStackData>>),
    MobDataRows(Vec<(crate::MobId, String)>),
    TerrainSpaces(Vec<crate::TerrainSpace>),
    MemoClaim(MemoClaim),
    TerrainHeights(Vec<i32>),
    MaybeU16(Option<u16>),
    SectionBlocks(#[serde(with = "serde_bytes")] Vec<u8>),
    Condition(Option<crate::ConditionInfoData>),
    BlockInfos(Vec<Option<BlockInfoData>>),
    AnimationClip(Option<crate::AnimationClipInfo>),
    BlockRecords(Vec<Option<crate::BlockRecord>>),
    RecordPlans(Vec<crate::RecordPlan>),
    RecordStatuses(Vec<crate::RecordStatus>),
    Dig(crate::DigProgress),
    Place(crate::PlaceRequest),
    Route(Option<crate::Route>),
    Schematic(crate::SchematicLookup),
    SchematicCells(Option<crate::SchematicCellsData>),
    Identity(Option<crate::PlayerIdentityData>),
    Flood(crate::Flood),
    Aims(Vec<Result<[f64; 3], crate::ActionRefusal>>),
    BlockChanges(crate::BlockChanges),
    Unsupported,
    ActingPlayer(Option<PlayerId>),
    PlayerOf(Option<Box<PlayerSnapshot>>),
    EffectsOf(Option<Vec<EffectStateData>>),
    Lights(Vec<Option<LightData>>),
    MobTagsMany(Vec<Option<Vec<(String, MobTagValue)>>>),
    PlayerInputs(Vec<Option<PlayerInputData>>),
    RidersMany(Vec<Option<MobRidersData>>),
    ClientViewState(ClientViewStateData),
    ClientContext(ClientContext),
    ClientStorageWrite(u64),
    ClientStorageWritten(bool),
    ClientEntities(Vec<ClientEntityData>),
    Ticket(u64),
    ClientStateTicket(crate::ClientStateTicketData),
    ClientFilePolled(Option<crate::ClientFileAnswer>),
    ClientFileStat(Option<crate::ClientFileInfo>),
    ClientEvents(Option<crate::ClientEventsReport>),
    ClientPresentationState(Box<crate::ClientPresentationStateData>),
    ClientCaptureStatus(crate::ClientCaptureStatus),
    ClientAudioTapState(Option<crate::ClientAudioTapData>),
    ClientMediaEncoders(Option<crate::ClientMediaCapabilities>),
    ClientMediaState(Option<Box<crate::ClientMediaStateData>>),
    ClientEngineFacts(crate::ClientEngineFactsData),
    ClientPacks(Vec<crate::ClientPackInfo>),
    ClientWallClock(crate::ClientWallTime),
    ClientFolder(Option<crate::ClientFolderInfo>),
    /// Heights as little-endian `i32`s, row by row: answers `TerrainHeightsIn`.
    TerrainHeightGrid(#[serde(with = "serde_bytes")] Vec<u8>),
}

impl HostRet {
    pub fn error(code: ErrorCode, detail: String) -> Self {
        Self::Err(HostError { code, detail })
    }

    pub fn refused(detail: impl Into<String>) -> Self {
        Self::error(ErrorCode::Refused, detail.into())
    }

    pub fn invalid(detail: String) -> Self {
        Self::error(ErrorCode::InvalidArgument, detail)
    }
}

pub fn decode_host_call(bytes: &[u8]) -> Result<crate::Decoded<HostCall>, postcard::Error> {
    let err = match crate::decode(bytes) {
        Ok(call) => return Ok(crate::Decoded::Known(call)),
        Err(e) => e,
    };
    let Ok((domain, rest)) = postcard::take_from_bytes::<u32>(bytes) else {
        return Err(err);
    };
    let Some((_, calls)) = HostCall::DOMAINS.get(domain as usize) else {
        return Ok(crate::Decoded::Unknown {
            domain: None,
            variant: domain,
        });
    };
    match postcard::take_from_bytes::<u32>(rest) {
        Ok((variant, _)) if variant as usize >= calls.len() => Ok(crate::Decoded::Unknown {
            domain: Some(domain),
            variant,
        }),
        _ => Err(err),
    }
}
