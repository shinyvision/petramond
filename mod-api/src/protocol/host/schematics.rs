use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    SchematicCall {
        SchematicInfo {
            asset: crate::SchematicId,
        } => legal(SERVER, Sim, Read),
        SchematicCells {
            asset: crate::SchematicId,
            section: u32,
            turns: u8,
        } => legal(SERVER, Sim, Read),
        SchematicChoose {
            player: PlayerId,
            tag: String,
        } => legal(SERVER, Sim, Write),
        SchematicPosition {
            player: PlayerId,
            tag: String,
            asset: crate::SchematicId,
            origin: Option<[i32; 3]>,
            turns: u8,
        } => legal(SERVER, Sim, Write),
        SchematicGhostSet {
            key: String,
            ghost: Option<crate::SchematicGhostData>,
        } => legal(SERVER, Sim, Write),
    }
}
