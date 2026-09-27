use crate::data::{ContainerAddress, EntityRef, ItemStackData};
use crate::legality::prelude::*;

host_domain! {
    ContainerCall {
        ContainerGet {
            at: ContainerAddress,
        } => legal(SERVER, Sim, Read),
        ContainerSet {
            at: ContainerAddress,
            slots: Vec<(u32, Option<ItemStackData>)>,
        } => legal(SERVER, Sim, Write),
        RecipeResult {
            class: String,
            item: String,
        } => legal(SERVER, Any, Read),
        ContainerGetMany {
            addresses: Vec<ContainerAddress>,
        } => legal(SERVER, Sim, Read),
        ContainerInsert {
            at: ContainerAddress,
            stack: ItemStackData,
        } => legal(SERVER, Sim, Write),
        ContainerTake {
            at: ContainerAddress,
            slot: u32,
            count: u8,
        } => legal(SERVER, Sim, Write),
        ContainerTransfer {
            from: ContainerAddress,
            slot: u32,
            to: ContainerAddress,
            count: u8,
        } => legal(SERVER, Sim, Write),
        ContainerHold {
            at: ContainerAddress,
            actor: EntityRef,
            open: bool,
        } => legal(SERVER, Sim, Write),
    }
}
