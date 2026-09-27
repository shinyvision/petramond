use crate::data::BlockRecord;
use crate::legality::prelude::*;

host_domain! {
    ConstructionCall {
        BlockRecordsAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        BlockRecordStatuses {
            cells: Vec<([i32; 3], BlockRecord)>,
        } => legal(SERVER, Sim, Read),
    }
}
