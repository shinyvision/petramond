use crate::data::{BlockRecord, EntityRef};
use crate::legality::prelude::*;

host_domain! {
    ActorCall {
        ActorDig {
            actor: EntityRef,
            pos: [i32; 3],
            tool_slot: Option<u32>,
            collect: bool,
        } => legal(SERVER, Sim, Write),
        ActorPlace {
            actor: EntityRef,
            pos: [i32; 3],
            record: BlockRecord,
            pay: bool,
        } => legal(SERVER, Sim, Write),
        ActorPlaceCheck {
            actor: EntityRef,
            from: [f64; 3],
            pos: [i32; 3],
            record: BlockRecord,
            pay: bool,
        } => legal(SERVER, Sim, Read),
        ActorInteract {
            actor: EntityRef,
            pos: [i32; 3],
        } => legal(SERVER, Sim, Write),
        ActorAims {
            actor: EntityRef,
            from: Vec<[f64; 3]>,
            pos: [i32; 3],
            record: Option<crate::BlockRecord>,
        } => legal(SERVER, Sim, Read),
    }
}
