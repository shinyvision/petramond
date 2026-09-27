use crate::data::EntityRef;
use crate::ids::ConditionId;
use crate::legality::prelude::*;

host_domain! {
    ConditionCall {
        EntityConditionApply {
            entity: EntityRef,
            condition: ConditionId,
            stage: u8,
            ticks: u32,
        } => legal(SERVER, Sim, Write),
        EntityConditionCool {
            entity: EntityRef,
            condition: ConditionId,
            ticks: u32,
        } => legal(SERVER, Sim, Write),
        EntityConditionsMany {
            ops: Vec<crate::ConditionOp>,
        } => legal(SERVER, Sim, Write),
    }
}
