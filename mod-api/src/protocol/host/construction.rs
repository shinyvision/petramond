//! Construction records: what placed blocks remember, and how a plan stands.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::BlockRecord;
use crate::legality::prelude::*;

host_domain! {
    /// Construction records: what placed blocks remember, and how a plan stands.
    ConstructionCall {
        /// Each position's cell as a [`BlockRecord`], parallel to `positions`:
        /// its row, shape state and carried data — never container contents or
        /// machine state. `None` = unloaded or not stream-final. At most
        /// `SIM_BATCH_MAX` positions. Server only. → [`HostRet::BlockRecords`](crate::HostRet::BlockRecords).
        BlockRecordsAt {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        /// Each record measured against the world at its position
        /// ([`RecordStatus`]), parallel to `cells`. At most `SIM_BATCH_MAX`
        /// cells. Server only. → [`HostRet::RecordStatuses`](crate::HostRet::RecordStatuses).
        BlockRecordStatuses {
            cells: Vec<([i32; 3], BlockRecord)>,
        } => legal(SERVER, Sim, Read),
    }
}
