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

/// Declare one host-call domain: its enum and the legality table the host's
/// gates read. Each variant is written as usual and followed by `=>` and its
/// [`Legality`], so a call and where it may be made are one declaration.
///
/// Generates `CALLS` (every call's name and legality, in wire order),
/// `name()` and `legality()`.
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
            /// Every call of this domain with its legality, in declaration
            /// (wire) order.
            pub const CALLS: &'static [$crate::CallInfo] = &[
                $($crate::CallInfo {
                    name: stringify!($variant),
                    legality: $legal,
                },)*
            ];

            /// The call's name, as [`Self::CALLS`] lists it.
            pub const fn name(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => stringify!($variant),)*
                }
            }

            /// Where the call is legal.
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

/// Declare [`HostCall`] over the domain enums: the wrapper enum, a `From`
/// per domain, the table-driven `name()`/`legality()`, and the flat
/// [`calls`] namespace.
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
            /// Every domain's call table, in wire order: `DOMAINS[d].1[c]` is
            /// the call encoded as `[d][c]`.
            pub const DOMAINS: &'static [(&'static str, &'static [CallInfo])] =
                &[$((stringify!($wrap), $domain::CALLS),)*];

            /// The call's name (its variant in its domain enum).
            pub const fn name(&self) -> &'static str {
                match self {
                    $(Self::$wrap(call) => call.name(),)*
                }
            }

            /// Where the call is legal — the one row every host gate reads.
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

        /// Every call of every domain under its own name —
        /// `calls::GetBlock { pos }` is a [`BlockCall`], and
        /// `HostCall::from` wraps it. The names are unique across domains, so
        /// a caller that only knows a call's name (the SDK's wrapper macro,
        /// a test) never has to know its domain.
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
        /// Instance basics: logging, the tick clock, the mod's RNG streams, the
        /// `mod_init` registration window, shader parameters, and a mod's own events.
        Core(CoreCall),
        /// The live world's cells: block reads and writes, light, columns, collision,
        /// placed-block presentation, raycasts, and the block change feed.
        Block(BlockCall),
        /// Live mobs and item entities: spawning, queries, damage, riding, drive and
        /// kinematic intents, named animations, navigation probes, and presentation.
        Entity(EntityCall),
        /// Authoritative player state: health, knockback, teleports, items, effects,
        /// inputs, progression, permissions, and the session roster.
        Player(PlayerCall),
        /// The acting player's body and rig: the calls BOTH sides serve — the server
        /// authoritatively, a client instance as a prediction against its own mirror
        /// (held and bone poses, displays, animator writes, the use gesture, the
        /// carried inventory, and the dispatch's actor).
        Body(BodyCall),
        /// Mod sounds and particle bursts: one-shot, spatial, mob-pinned, retuned,
        /// stopped.
        Sound(SoundCall),
        /// Persistent mod key/value state: world-level and per section cell.
        Kv(KvCall),
        /// The per-mob typed tag map.
        Tag(TagCall),
        /// The process-wide registries: name/id resolution both ways, tag membership,
        /// rows and instance data, structures, loot, conditions, record plans. Legal on
        /// every instance at any time.
        Registry(RegistryCall),
        /// Worldgen hooks and the pure positional terrain queries: gen registrations,
        /// the underground-biome partition, terrain columns, sections and spaces — pure
        /// functions of (world seed, position), legal on the detached worldgen
        /// instances.
        Worldgen(WorldgenCall),
        /// The shared derived-fact memo, scoped to (mod, world seed): settled
        /// positional facts one instance derives and every instance of the mod reuses.
        Memo(MemoCall),
        /// Mod GUIs: session state, opening and closing, viewers.
        Gui(GuiCall),
        /// Container slots: reads, writes, admission-ruled inserts and takes,
        /// transfers, holds, and recipe results.
        Container(ContainerCall),
        /// The presentation-only client surface: overlays, keys, images, text, GUIs,
        /// canvases, sandboxed storage, environment and ambience, and the read-only
        /// replica queries. Client instances only.
        Client(ClientCall),
        /// Dropped and launched item entities in motion: radius queries and impulses.
        ItemMotion(ItemMotionCall),
        /// Body conditions on live players and mobs: grants and cooling.
        Condition(ConditionCall),
        /// Construction records: what placed blocks remember, and how a plan stands.
        Construction(ConstructionCall),
        /// Mob actors acting on the world: digging, placing, interacting, aiming.
        Actor(ActorCall),
        /// Schematics: catalog reads, cell lists, player choices and positioning, ghosts.
        Schematic(SchematicCall),
        /// A client mod's own files in its storage buckets: writes, sync, rename,
        /// delete, reads, listings and stats, all ticketed.
        ClientFile(ClientFileCall),
        /// The presented world's state and events, copied into mod files.
        ClientCapture(ClientCaptureCall),
        /// A world presented from mod-file byte ranges, opened from the shell.
        ClientPresentation(ClientPresentationCall),
        /// Rendered frames, the stepped clock, taps on the world's sound, and
        /// media files encoded into mod storage.
        ClientMedia(ClientMediaCall),
    }
}

/// The three ways a [`MemoCall::MemoClaim`] comes back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum MemoClaim {
    /// The published value.
    Value(#[serde(with = "serde_bytes")] Vec<u8>),
    /// Nobody held it: this caller does now, and must publish with
    /// [`MemoCall::MemoPut`].
    Lease,
    /// Another caller holds the lease and has not published within the
    /// short wait. A generation callback answers with a deferred
    /// [`GenOutput`](crate::GenOutput) so the section is dispatched again
    /// once the value exists; anything else derives the fact itself.
    Pending,
}

/// Host → guest reply for a [`HostCall`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum HostRet {
    Unit,
    U64(u64),
    /// The call was refused, with why ([`HostError`]). The SDK surfaces it as
    /// a `Result` where the failure depends on data, and as a guest panic —
    /// the mod disabled — where it can only be a mod bug.
    Err(HostError),
    Bool(bool),
    /// [`BlockCall::GetBlock`]: `None` = section unloaded / out of range.
    Block(Option<BlockId>),
    /// [`BlockCall::GetBlocks`] / [`ClientCall::ClientBlocksAt`], parallel to
    /// the request positions.
    Blocks(Vec<Option<BlockId>>),
    /// [`BlockCall::LightAt`], all on the 6-bit `0..=63` scale. `None` =
    /// section unloaded / streamed content not final (never fabricated).
    Light(Option<LightData>),
    /// [`EntityCall::MobsInRadius`] / [`TagCall::MobsWithTag`].
    Mobs(Vec<MobSnapshot>),
    /// [`BodyCall::PlayerState`].
    Player(Box<PlayerSnapshot>),
    /// The KV gets: `None` = key absent (or target unloaded/missing).
    Bytes(#[serde(with = "serde_bytes")] Option<Vec<u8>>),
    /// [`TagCall::MobTagGet`]: the lookup outcome — a missing mob is told
    /// apart from an absent key (see [`MobTagLookup`]).
    MobTag(MobTagLookup),
    /// [`GuiCall::GuiStateGet`]: `None` = key absent.
    GuiValue(Option<GuiValue>),
    /// [`ContainerCall::ContainerGet`]: every slot in index order; `None` = no
    /// container / unloaded.
    ContainerSlots(Option<Vec<Option<ItemStackData>>>),
    /// [`RegistryCall::ItemInfo`]: `None` = unknown item key.
    ///
    /// BOXED: `ItemInfoData` is by far the largest thing this enum carries, and
    /// unboxed it set the size of EVERY host-call reply — including the guards'
    /// `Result<_, HostRet>` rejection path, which is on every validated call.
    /// `serde` treats `Box<T>` exactly as `T`, so the wire is unchanged
    /// (`wire_pin` proves it).
    ItemInfo(Option<Box<ItemInfoData>>),
    /// [`ContainerCall::RecipeResult`]: `None` = no recipe for that input.
    ItemStack(Option<ItemStackData>),
    /// [`PlayerCall::EffectsActive`]: the player's active status effects.
    Effects(Vec<EffectStateData>),
    /// [`ContainerCall::ContainerGetMany`], parallel to the request positions
    /// (each entry as [`HostRet::ContainerSlots`]'s payload).
    Containers(Vec<Option<Vec<Option<ItemStackData>>>>),
    RuntimeSide(RuntimeSide),
    /// [`ClientCall::ClientSurfaceColumns`], parallel to the request queries:
    /// `None` = column unknown to the replica; a reply without cell bytes =
    /// unchanged since the queried revision.
    ClientSurfaceColumns(Vec<Option<ClientSurfaceColumn>>),
    ClientTextSize([u16; 2]),
    ClientStorageValues(Vec<Option<serde_bytes::ByteBuf>>),
    /// [`RegistryCall::ResolveItem`]: `None` = unknown item name.
    Item(Option<ItemId>),
    /// [`ClientCall::ClientStorageReadPoll`]: `None` = still in flight (poll
    /// again next frame); `Some` consumes the ticket.
    ClientStorageRead(Option<Vec<Option<serde_bytes::ByteBuf>>>),
    /// [`EntityCall::MobRiders`]: `None` = no such live mob.
    Riders(Option<MobRidersData>),
    /// [`EntityCall::BlockModelGroup`]: `None` = no model group / unloaded.
    ModelGroup(Option<crate::ModelGroupData>),
    /// [`PlayerCall::PlayerInput`]: `None` = no such player connected.
    PlayerInput(Option<PlayerInputData>),
    /// [`EntityCall::MobAnimState`]: `None` = missing/dead mob or inactive anim.
    MobAnimState(Option<MobAnimStateData>),
    /// Byte-vocabulary answers (biome ids): `None` = unloaded/unknown.
    MaybeByte(Option<u8>),
    /// [`BlockCall::SurfaceYAt`]: `None` = unloaded or all-air column.
    MaybeI32(Option<i32>),
    /// [`PlayerCall::Players`]: every connected player, session-id order.
    Players(Vec<PlayerListEntry>),
    /// [`ClientCall::ClientEnvParams`], parallel to the request keys
    /// (`None` = param not present in the environment).
    EnvParams(Vec<Option<[f32; 4]>>),
    /// [`RegistryCall::BlocksByTag`]: the tag's members, id order (empty = no
    /// block carries it).
    BlockList(Vec<BlockId>),
    /// [`RegistryCall::ItemsByTag`]: the tag's members, id order (empty = no
    /// item carries it).
    ItemList(Vec<ItemId>),
    /// [`RegistryCall::BlockNames`] / [`RegistryCall::ItemNames`] /
    /// [`RegistryCall::MobNames`], parallel to the request ids (`None` =
    /// unregistered id).
    Names(Vec<Option<String>>),
    /// [`RegistryCall::ResolveMob`]: `None` = unregistered species key.
    MobKind(Option<MobId>),
    /// [`BlockCall::CollisionShapeAt`]: `None` = section unloaded / streamed
    /// content not final.
    CollisionShape(Option<CollisionShape>),
    /// [`TagCall::MobTagsGet`]: the mob's full tag map, sorted by key;
    /// `None` = no such live mob.
    MobTags(Option<Vec<(String, MobTagValue)>>),
    /// [`EntityCall::SpawnMob`]: the newborn's STABLE session id — the address
    /// every mob call speaks, so a spawner can immediately tag/configure what
    /// it created. `None` = unknown key or a failed check.
    SpawnedMob(Option<u64>),
    /// [`BlockCall::FindBlocks`]: matching cells in scan order; `None` = some
    /// cell in the box is unloaded / streamed content not yet final.
    FoundBlocks(Option<Vec<[i32; 3]>>),
    /// [`EntityCall::MobInfo`]: the mob's snapshot; `None` = no such live mob.
    Mob(Option<MobSnapshot>),
    /// [`EntityCall::ItemEntity`]: the item entity's snapshot; `None` = no
    /// such live entity.
    ItemEntity(Option<Box<ItemEntityData>>),
    /// [`ClientCall::ClientCellKvAt`]: one value per requested cell, parallel
    /// to the request (`None` = absent / cell unknown).
    BytesMany(Vec<Option<Vec<u8>>>),
    /// [`RegistryCall::ItemsWithData`]: every carrying item with its raw JSON
    /// value, in id order.
    ItemDataRows(Vec<(ItemId, String)>),
    /// [`RegistryCall::BlocksWithData`]: every carrying block with its raw JSON
    /// value, in id order.
    BlockDataRows(Vec<(BlockId, String)>),
    /// [`WorldgenCall::UndergroundBiomeAt`]: one id per requested position, in
    /// order.
    UndergroundBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    /// [`WorldgenCall::TerrainSolidAt`]: one flag per requested position, in
    /// order.
    TerrainSolid(Vec<bool>),
    /// [`WorldgenCall::SurfaceBiomeAt`]: one biome id per requested column, in
    /// order.
    SurfaceBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    /// [`BlockCall::BlockLocalToWorld`]: one world point per requested point,
    /// in order. `None` = the addressed cell is unloaded or not stream-final.
    Points(Option<Vec<[f64; 3]>>),
    /// The batched WRITE replies ([`BlockCall::SetBlockDraws`],
    /// [`BlockCall::SetModelPartsMany`], [`KvCall::SectionKvSetMany`]):
    /// one flag per requested entry, in order, meaning exactly what the
    /// single call's `Bool` means.
    Bools(Vec<bool>),
    /// [`GuiCall::GuiViewers`]: every session with a mod GUI open.
    GuiViewers(Vec<GuiViewerData>),
    /// [`RegistryCall::BlockInfo`]: `None` = unregistered id. Boxed like
    /// [`HostRet::ItemInfo`], for the same reply-size reason.
    BlockInfo(Option<Box<BlockInfoData>>),
    /// [`PlayerCall::PlayerHeld`]: `None` = empty hand / no such session.
    HeldStack(Option<ItemStackData>),
    /// [`BlockCall::Raycast`]: the first block the ray stops on; `None` =
    /// nothing within `max`.
    Raycast(Option<RaycastHitData>),
    /// [`ItemMotionCall::ItemEntitiesInRadius`], nearest first, stable id breaks ties.
    ItemEntities(Vec<ItemEntityData>),
    StructureInfo(Option<Box<crate::StructureInfoData>>),
    /// Undelivered reward stacks from [`RegistryCall::LootRoll`].
    Loot(Option<Vec<ItemStackData>>),
    MobDataRows(Vec<(crate::MobId, String)>),
    TerrainSpaces(Vec<crate::TerrainSpace>),
    /// [`MemoCall::MemoClaim`].
    MemoClaim(MemoClaim),
    TerrainHeights(Vec<i32>),
    MaybeU16(Option<u16>),
    /// [`WorldgenCall::TerrainSectionAt`]: the 4,096 ids as little-endian pairs
    /// in section order, copied rather than encoded one by one.
    SectionBlocks(#[serde(with = "serde_bytes")] Vec<u8>),
    /// [`RegistryCall::ResolveCondition`].
    Condition(Option<crate::ConditionInfoData>),
    /// [`RegistryCall::BlockInfos`], parallel to the request.
    BlockInfos(Vec<Option<BlockInfoData>>),
    /// [`BodyCall::AnimationClip`].
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
    /// [`BlockCall::BlockChangesSince`].
    BlockChanges(crate::BlockChanges),
    /// The host does not know the call: the guest was built against a newer
    /// ABI minor than the host speaks (see [`decode_host_call`]). The SDK's
    /// wrappers treat it like any unexpected reply; a mod that wants to degrade
    /// gracefully checks `mod_sdk::host_supports` before calling.
    Unsupported,
    /// [`BodyCall::ActingPlayer`]: `None` = an actor-less dispatch.
    ActingPlayer(Option<PlayerId>),
    /// [`PlayerCall::PlayerStateOf`]: `None` = no such connected session.
    PlayerOf(Option<Box<PlayerSnapshot>>),
    /// [`PlayerCall::EffectsActiveOf`]: `None` = no such connected session.
    EffectsOf(Option<Vec<EffectStateData>>),
    /// [`BlockCall::LightAtMany`], parallel to the request positions (each
    /// entry as [`HostRet::Light`]'s payload).
    Lights(Vec<Option<LightData>>),
    /// [`TagCall::MobTagsGetMany`], parallel to the request ids (each entry
    /// as [`HostRet::MobTags`]'s payload).
    MobTagsMany(Vec<Option<Vec<(String, MobTagValue)>>>),
    /// [`PlayerCall::PlayerInputs`], parallel to the request ids (each entry
    /// as [`HostRet::PlayerInput`]'s payload).
    PlayerInputs(Vec<Option<PlayerInputData>>),
    /// [`EntityCall::MobRidersMany`], parallel to the request ids (each entry
    /// as [`HostRet::Riders`]'s payload).
    RidersMany(Vec<Option<MobRidersData>>),
    /// [`ClientCall::ClientViewState`].
    ClientViewState(ClientViewStateData),
    /// [`ClientCall::ClientContext`].
    ClientContext(ClientContext),
    /// [`ClientCall::ClientStorageSetMany`]: the write's ticket. A refusal is
    /// [`HostRet::Err`] with [`ErrorCode::Refused`].
    ClientStorageWrite(u64),
    /// [`ClientCall::ClientStorageWritePoll`]: `true` = on disk, `false` =
    /// still queued. A write the disk refused answers [`ErrorCode::Refused`].
    ClientStorageWritten(bool),
    /// [`ClientCall::ClientEntities`].
    ClientEntities(Vec<ClientEntityData>),
    /// A ticketed call, accepted: its id. A refusal, with a reason a player
    /// can read, is [`HostRet::Err`] with [`ErrorCode::Refused`] — an answer,
    /// never a trap.
    Ticket(u64),
    /// [`ClientCaptureCall::ClientWorldStateWrite`], accepted.
    ClientStateTicket(crate::ClientStateTicketData),
    /// [`ClientFileCall::ClientFilePoll`]: `None` = not finished. A ticket
    /// that ended in failure answers [`ErrorCode::Refused`] with its reason.
    ClientFilePolled(Option<crate::ClientFileAnswer>),
    /// [`ClientFileCall::ClientFileStat`]: `None` = no such file.
    ClientFileStat(Option<crate::ClientFileInfo>),
    /// [`ClientCaptureCall::ClientWorldEventsPoll`]: `None` = no such log.
    ClientEvents(Option<crate::ClientEventsReport>),
    /// [`ClientPresentationCall::ClientPresentationState`]. Boxed only to keep
    /// `HostRet` small; the wire is the same.
    ClientPresentationState(Box<crate::ClientPresentationStateData>),
    /// [`ClientMediaCall::ClientFrameCapturePoll`].
    ClientCaptureStatus(crate::ClientCaptureStatus),
    /// [`ClientMediaCall::ClientAudioTapState`]: `None` = no such tap.
    ClientAudioTapState(Option<crate::ClientAudioTapData>),
    /// [`ClientMediaCall::ClientMediaEncoders`]: `None` = still asking.
    ClientMediaEncoders(Option<crate::ClientMediaCapabilities>),
    /// [`ClientMediaCall::ClientMediaState`]: `None` = no such media file.
    /// Boxed only to keep `HostRet` small.
    ClientMediaState(Option<Box<crate::ClientMediaStateData>>),
    /// [`ClientCall::ClientEngineFacts`].
    ClientEngineFacts(crate::ClientEngineFactsData),
    /// [`ClientCall::ClientPacks`].
    ClientPacks(Vec<crate::ClientPackInfo>),
    /// [`ClientCall::ClientWallClock`].
    ClientWallClock(crate::ClientWallTime),
}

impl HostRet {
    /// A refusal: [`HostRet::Err`] with `code` and a human-readable `detail`.
    pub fn error(code: ErrorCode, detail: String) -> Self {
        Self::Err(HostError { code, detail })
    }

    /// A refusal a player can read ([`ErrorCode::Refused`]).
    pub fn refused(detail: impl Into<String>) -> Self {
        Self::error(ErrorCode::Refused, detail.into())
    }

    /// A refusal for a malformed argument ([`ErrorCode::InvalidArgument`]) —
    /// the common case, so it gets the short spelling.
    pub fn invalid(detail: String) -> Self {
        Self::error(ErrorCode::InvalidArgument, detail)
    }
}

/// Decode a [`HostCall`], telling an unknown-but-well-framed call apart from
/// a malformed buffer at BOTH levels of the nesting: a domain index past the
/// last domain, or a call index past the end of a known domain, is a call
/// from a newer ABI minor (answer [`HostRet::Unsupported`]); anything else
/// that fails to decode is a broken peer.
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
