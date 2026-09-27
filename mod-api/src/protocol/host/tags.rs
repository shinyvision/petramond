use crate::data::MobTagValue;
use crate::legality::prelude::*;

host_domain! {
    TagCall {
        MobTagGet {
            mob_id: u64,
            key: String,
        } => legal(SERVER, Sim, Read),
        MobTagSet {
            mob_id: u64,
            key: String,
            value: MobTagValue,
        } => legal(SERVER, Sim, Write),
        MobTagDelete {
            mob_id: u64,
            key: String,
        } => legal(SERVER, Sim, Write),
        MobTagsGet {
            mob_id: u64,
        } => legal(SERVER, Sim, Read),
        MobsWithTag {
            key: String,
            value: Option<MobTagValue>,
        } => legal(SERVER, Sim, Read),
        MobTagsGetMany {
            mob_ids: Vec<u64>,
        } => legal(SERVER, Sim, Read),
        MobTagsWrite {
            writes: Vec<crate::MobTagOp>,
        } => legal(SERVER, Sim, Write),
    }
}
