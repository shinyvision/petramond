//! Construction reads: world cells as portable block records, and records
//! measured against the world. (What a record costs on its own is a registry
//! read: [`crate::block_record_plans`].)

use mod_api::{BlockRecord, RecordStatus};

use crate::__rt::host_fn;

host_fn! {
    /// Each cell as a [`BlockRecord`] — row, shape state and carried data,
    /// never container contents or machine state — parallel to `positions`.
    /// `None` = unloaded or not stream-final. At most
    /// [`crate::SIM_BATCH_MAX`] positions. Server only.
    pub fn block_records_at(positions: Vec<[i32; 3]>) -> Vec<Option<BlockRecord>>
        => BlockRecordsAt { positions } => BlockRecords
}

host_fn! {
    /// Each record measured against the world at its position, parallel to
    /// `cells` (at most [`crate::SIM_BATCH_MAX`]). Server only.
    pub fn block_record_statuses(cells: Vec<([i32; 3], BlockRecord)>) -> Vec<RecordStatus>
        => BlockRecordStatuses { cells } => RecordStatuses
}

host_fn! {
    /// One tick of `actor` digging the block at `pos` with the tool in
    /// `tool_slot` of its own carried slots (`None` = bare hands). Call it on
    /// consecutive ticks until it answers [`mod_api::DigProgress::Breaking`];
    /// the break's outcome arrives as `actor_acted`. With `collect` the drops
    /// go into the actor's slots first. Server only.
    pub fn actor_dig(actor: mod_api::EntityRef, pos: [i32; 3], tool_slot: Option<u32>, collect: bool) -> mod_api::DigProgress
        => ActorDig { actor, pos, tool_slot, collect } => Dig
}

host_fn! {
    /// `actor` builds `record` at `pos` under survival placement rules,
    /// paying from its own carried slots when `pay` (without `pay` the block
    /// must be this mod's own). A queued placement's outcome arrives as
    /// `actor_acted`. Server only.
    pub fn actor_place(actor: mod_api::EntityRef, pos: [i32; 3], record: BlockRecord, pay: bool) -> mod_api::PlaceRequest
        => ActorPlace { actor, pos, record, pay } => Place
}

host_fn! {
    /// What [`actor_place`] would answer with `actor`'s feet at `from`,
    /// placing nothing: `Queued` = the world would accept it. Server only.
    pub fn actor_place_check(actor: mod_api::EntityRef, from: [f64; 3], pos: [i32; 3], record: BlockRecord, pay: bool) -> mod_api::PlaceRequest
        => ActorPlaceCheck { actor, from, pos, record, pay } => Place
}

host_fn! {
    /// `actor` uses the block at `pos` as a player's right-click would, for
    /// what a body does without a screen (a door swings). `false` = nothing
    /// to use there or out of reach. Server only.
    pub fn actor_interact(actor: mod_api::EntityRef, pos: [i32; 3]) -> bool
        => ActorInteract { actor, pos } => Bool
}

host_fn! {
    /// Where `actor`, with its feet at each of `from`, would look to place
    /// `record` at `pos` — or, without a record, to dig or use the block
    /// there. Parallel to `from` (at most [`crate::SIM_BATCH_MAX`]). An
    /// actor's actions land only where it is looking. Server only.
    pub fn actor_aims(actor: mod_api::EntityRef, from: Vec<[f64; 3]>, pos: [i32; 3], record: Option<BlockRecord>) -> Vec<Result<[f64; 3], mod_api::ActionRefusal>>
        => ActorAims { actor, from, pos, record } => Aims
}
