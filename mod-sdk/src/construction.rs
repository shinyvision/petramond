use mod_api::{BlockRecord, RecordStatus};

use crate::__rt::host_fn;

host_fn! {
    pub fn block_records_at(positions: Vec<[i32; 3]>) -> Vec<Option<BlockRecord>>
        => BlockRecordsAt { positions } => BlockRecords
}

host_fn! {
    pub fn block_record_statuses(cells: Vec<([i32; 3], BlockRecord)>) -> Vec<RecordStatus>
        => BlockRecordStatuses { cells } => RecordStatuses
}

host_fn! {
    pub fn actor_dig(actor: mod_api::EntityRef, pos: [i32; 3], tool_slot: Option<u32>, collect: bool) -> mod_api::DigProgress
        => ActorDig { actor, pos, tool_slot, collect } => Dig
}

host_fn! {
    pub fn actor_place(actor: mod_api::EntityRef, pos: [i32; 3], record: BlockRecord, pay: bool) -> mod_api::PlaceRequest
        => ActorPlace { actor, pos, record, pay } => Place
}

host_fn! {
    pub fn actor_place_check(actor: mod_api::EntityRef, from: [f64; 3], pos: [i32; 3], record: BlockRecord, pay: bool) -> mod_api::PlaceRequest
        => ActorPlaceCheck { actor, from, pos, record, pay } => Place
}

host_fn! {
    pub fn actor_interact(actor: mod_api::EntityRef, pos: [i32; 3]) -> bool
        => ActorInteract { actor, pos } => Bool
}

host_fn! {
    pub fn actor_aims(actor: mod_api::EntityRef, from: Vec<[f64; 3]>, pos: [i32; 3], record: Option<BlockRecord>) -> Vec<Result<[f64; 3], mod_api::ActionRefusal>>
        => ActorAims { actor, from, pos, record } => Aims
}
