//! Construction calls: cells read as portable records and records measured
//! against the world. What a record costs on its own is registry-only and
//! lives in the registry domain; the live-world questions live here.

use mod_api::{BlockRecord, HostCall, HostRet, RecordPlan, RecordStatus};
use petramond_math::math::IVec3;
use petramond_world::construction::{Plan, Record};

use super::guards::{batch_guard, item_stack_data, sim_query};
use crate::schematic::CellData;
use crate::world::construction::CellStatus;

pub(super) fn handle_construction_call(call: HostCall) -> HostRet {
    match call {
        HostCall::BlockRecordsAt { positions } => {
            if let Some(err) = batch_guard("BlockRecordsAt position", positions.len()) {
                return err;
            }
            sim_query(|ctx| {
                HostRet::BlockRecords(
                    positions
                        .iter()
                        .map(|&p| {
                            ctx.world
                                .construction_record(IVec3::from_array(p))
                                .map(|r| record_out(&r))
                        })
                        .collect(),
                )
            })
        }
        HostCall::BlockRecordStatuses { cells } => {
            if let Some(err) = batch_guard("BlockRecordStatuses cell", cells.len()) {
                return err;
            }
            sim_query(|ctx| {
                HostRet::RecordStatuses(
                    cells
                        .iter()
                        .map(|(pos, record)| match record_in(record) {
                            Err(reason) => RecordStatus::Unsupported { reason },
                            Ok(record) => status_out(
                                ctx.world
                                    .construction_status(IVec3::from_array(*pos), &record),
                            ),
                        })
                        .collect(),
                )
            })
        }
        other => HostRet::Error(format!(
            "non-construction call {other:?} mis-routed to handle_construction_call (host bug)"
        )),
    }
}

/// A record in names, resolved and stripped to what construction builds.
pub(in crate::modding) fn record_in(record: &BlockRecord) -> Result<Record, String> {
    CellData {
        block: record.block.clone(),
        state: record.state.clone(),
        state_ids: record.refs.iter().cloned().collect(),
        fluid: 0,
        kv: record.data.iter().cloned().collect(),
        container: None,
        furnace: None,
    }
    .construction_record()
}

pub(in crate::modding) fn record_out(record: &Record) -> BlockRecord {
    let named = CellData::of_record(record);
    BlockRecord {
        block: named.block,
        state: named.state,
        refs: named.state_ids.into_iter().collect(),
        data: named.kv.into_iter().collect(),
    }
}

/// What `record` asks of construction, offsets relative to its own cell.
pub(in crate::modding) fn plan_out(record: &BlockRecord) -> RecordPlan {
    let record = match record_in(record) {
        Ok(record) => record,
        Err(reason) => return RecordPlan::Unsupported { reason },
    };
    match petramond_world::construction::plan(&record, IVec3::ZERO) {
        Plan::Air => RecordPlan::Air,
        Plan::Member(anchor) => RecordPlan::Member {
            anchor: anchor.to_array(),
        },
        Plan::Unit { cost, writes } => RecordPlan::Unit {
            cost: cost.into_iter().map(item_stack_data).collect(),
            footprint: writes.writes.iter().map(|w| w.cell.to_array()).collect(),
        },
        Plan::Unsupported(reason) => RecordPlan::Unsupported { reason },
    }
}

fn status_out(status: CellStatus) -> RecordStatus {
    match status {
        CellStatus::Unloaded => RecordStatus::Unloaded,
        CellStatus::Satisfied => RecordStatus::Satisfied,
        CellStatus::Place { missing, .. } => RecordStatus::Place {
            missing: missing.into_iter().map(item_stack_data).collect(),
        },
        CellStatus::Clear {
            at,
            block,
            footprint,
            holds_items,
        } => RecordStatus::Clear {
            at: at.to_array(),
            block: mod_api::BlockId(block.id()),
            footprint: footprint.iter().map(|c| c.to_array()).collect(),
            holds_items,
        },
        CellStatus::Pending(anchor) => RecordStatus::Pending {
            anchor: anchor.to_array(),
        },
        CellStatus::Unsupported(reason) => RecordStatus::Unsupported { reason },
    }
}

#[cfg(test)]
mod tests;
