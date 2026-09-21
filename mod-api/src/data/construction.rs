//! Construction and actor vocabulary: portable cell records, what building
//! them asks, how a mob's action is answered, routes, the change log, and
//! world-held schematics.

use serde::{Deserialize, Serialize};

use super::*;

/// A portable description of one cell, in names: its block row, its shape
/// state (embedded block references zeroed in `state` and named in `refs` by
/// byte offset), and the cell data its item carries in. World reads and
/// schematic reads both answer in it; the state bytes are opaque to mods,
/// which store, compare and hand records back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockRecord {
    pub block: String,
    #[serde(with = "serde_bytes")]
    pub state: Vec<u8>,
    pub refs: Vec<(u8, String)>,
    pub data: Vec<(String, Vec<u8>)>,
}

/// What a [`BlockRecord`] asks of construction on its own. Offsets are
/// relative to the record's cell.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RecordPlan {
    /// The cell must be empty: clearance, never items.
    Air,
    /// Part of an object anchored at `anchor`, which builds it.
    Member { anchor: [i32; 3] },
    /// An object this cell anchors: what it costs and every cell it fills.
    Unit {
        cost: Vec<ItemStackData>,
        footprint: Vec<[i32; 3]>,
    },
    /// Items cannot build it (an unknown row, a fluid, a row declaring so).
    Unsupported { reason: String },
}

/// A [`BlockRecord`] measured against the world at a position.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RecordStatus {
    /// A cell the object needs is unloaded or not stream-final: retry later.
    Unloaded,
    /// The world already holds it (neighbour-derived shape and state a player
    /// toggles in use, like a door standing open, do not count against it).
    Satisfied,
    /// Build it, paying `missing` — only the missing parts of a partly built
    /// cell.
    Place {
        missing: Vec<ItemStackData>,
    },
    /// `block` occupies `at`; breaking it takes the whole `footprint`, and
    /// `holds_items` says it is a container that is not empty.
    Clear {
        at: [i32; 3],
        block: BlockId,
        footprint: Vec<[i32; 3]>,
        holds_items: bool,
    },
    /// A member cell waiting for the object anchored at `anchor`.
    Pending {
        anchor: [i32; 3],
    },
    Unsupported {
        reason: String,
    },
}

/// Why an actor's world action did not happen (see [`HostCall::ActorDig`]
/// and [`HostCall::ActorPlace`]).
///
/// [`HostCall::ActorDig`]: crate::HostCall::ActorDig
/// [`HostCall::ActorPlace`]: crate::HostCall::ActorPlace
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ActionRefusal {
    /// The actor is not a live mob.
    NoActor,
    /// A cell the action needs is unloaded or not stream-final.
    Unloaded,
    /// The target lies beyond the actor's reach, measured from its eye.
    OutOfReach,
    /// A colliding block stands between the actor's eye and the target.
    NoLineOfSight,
    /// Nothing to do: no block to dig, or the record is already built.
    NothingToDo,
    /// The block cannot be broken.
    Unbreakable,
    /// A block occupies a cell the placement needs.
    Obstructed,
    /// No block beside the object to place it against.
    NoFace,
    /// The world does not hold the object up.
    NoSupport,
    /// A body stands where the object would collide.
    BodyInTheWay,
    /// The actor does not carry what the placement costs.
    MissingItems,
    /// The named tool slot is empty or does not exist.
    NoTool,
    /// Items cannot build the record.
    Unsupported,
    /// A payment-free placement of a block the calling mod does not own.
    NotOwned,
    /// A pre-event handler cancelled the action.
    Vetoed,
    /// The target changed between the request and its turn.
    Changed,
    /// The actor is not looking at it: its gaze lands elsewhere, or on a
    /// face that would not place this (see `HostCall::ActorAims`).
    NotAimed,
    /// Every face seen from there places the object turned another way.
    Misaligned,
}

/// Where an actor's dig stands after one [`HostCall::ActorDig`] tick.
///
/// [`HostCall::ActorDig`]: crate::HostCall::ActorDig
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum DigProgress {
    /// Still digging: `progress` of the break's full duration, 0..1.
    Digging {
        progress: f32,
    },
    /// The break finished its duration and is queued for this tick; its
    /// outcome arrives as [`EventKind::ActorActed`](crate::EventKind::ActorActed).
    Breaking,
    Refused(ActionRefusal),
}

/// The answer to one [`HostCall::ActorPlace`].
///
/// [`HostCall::ActorPlace`]: crate::HostCall::ActorPlace
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum PlaceRequest {
    /// Valid now and queued for this tick; its outcome arrives as
    /// [`EventKind::ActorActed`](crate::EventKind::ActorActed).
    Queued,
    /// The world already holds the record: nothing to pay, nothing queued.
    Satisfied,
    Refused(ActionRefusal),
}

/// What one bounded route search learned (see [`HostCall::PathProbe`]).
///
/// [`HostCall::PathProbe`]: crate::HostCall::PathProbe
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// The body can walk there.
    Open,
    /// It cannot: everywhere the body could walk was searched.
    Closed,
    /// The search spent its expansions before deciding either way.
    Undecided,
}

/// What one bounded walkable-region flood learned (see [`HostCall::WalkRegion`]).
///
/// [`HostCall::WalkRegion`]: crate::HostCall::WalkRegion
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Flood {
    /// Every foothold reached, breadth first, each with the moves between it
    /// and the flood's start (a step, a climb or a drop is one move).
    Reached(Vec<([i32; 3], u32)>),
    /// The box holds more footholds than were asked for; asking again the
    /// same way gets the same answer.
    Exceeded,
    /// This tick's route budget cannot cover the flood: ask again next tick.
    Deferred,
}

/// A stretch of the world's change log (see [`HostCall::BlockChangesSince`]).
///
/// [`HostCall::BlockChangesSince`]: crate::HostCall::BlockChangesSince
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockChanges {
    /// The entry to ask from next time.
    pub next: u64,
    /// Entries asked for are gone: every cell may have changed.
    pub lost: bool,
    /// The cells changed, oldest first; one may appear more than once.
    pub cells: Vec<[i32; 3]>,
}

/// Which actor action an [`EventKind::ActorActed`](crate::EventKind::ActorActed) reports.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ActorAction {
    Dig,
    Place,
    /// Used the block ([`HostCall::ActorInteract`](crate::HostCall::ActorInteract)).
    Use,
}

/// A schematic asset's identity: the BLAKE3 digest of its complete archive
/// bytes. The same design from anyone is the same asset.
pub type SchematicId = [u8; 32];

/// A world-held schematic's facts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SchematicInfoData {
    pub title: String,
    /// Width, height and depth before any turn.
    pub size: [i32; 3],
    /// Stored cells (selected positions, explicit air included).
    pub cells: u64,
    /// Stored sections, each read with [`HostCall::SchematicCells`].
    ///
    /// [`HostCall::SchematicCells`]: crate::HostCall::SchematicCells
    pub sections: u32,
}

/// Where a schematic stands for a reader ([`HostCall::SchematicInfo`]).
///
/// [`HostCall::SchematicInfo`]: crate::HostCall::SchematicInfo
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SchematicLookup {
    /// The world holds no such asset.
    Missing,
    /// Decoding in the background: ask again on a later tick.
    Loading,
    Ready(SchematicInfoData),
    Failed {
        reason: String,
    },
}

/// One stored section of a schematic, turned: each cell's position inside
/// the turned design (its minimum corner at the origin) and an index into
/// `palette`, the section's construction records.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SchematicCellsData {
    pub cells: Vec<([i32; 3], u16)>,
    pub palette: Vec<BlockRecord>,
}

/// A retained anchored ghost ([`HostCall::SchematicGhostSet`]).
///
/// [`HostCall::SchematicGhostSet`]: crate::HostCall::SchematicGhostSet
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SchematicGhostData {
    pub asset: SchematicId,
    /// The turned design's minimum corner.
    pub origin: [i32; 3],
    pub turns: u8,
    /// Who sees it; empty = every player.
    pub viewers: Vec<PlayerId>,
    /// A viewer positioning a schematic under this ghost's own key
    /// ([`HostCall::SchematicPosition`](crate::HostCall::SchematicPosition),
    /// `tag` = the key) does not see it meanwhile: the old placement steps
    /// aside for the one being chosen, and returns if they back out.
    pub yields_to_positioning: bool,
}
