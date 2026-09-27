use crate::legality::prelude::*;

host_domain! {
    KvCall {
        WorldKvGet {
            key: String,
        } => legal(SERVER, Sim, Read),
        WorldKvSet {
            key: String,
            value: Vec<u8>,
        } => legal(SERVER, Sim, Write),
        WorldKvDelete {
            key: String,
        } => legal(SERVER, Sim, Write),
        SectionKvGet {
            pos: [i32; 3],
            key: String,
        } => legal(SERVER, Sim, Read),
        SectionKvSet {
            pos: [i32; 3],
            key: String,
            value: Vec<u8>,
        } => legal(SERVER, Sim, Write),
        SectionKvDelete {
            pos: [i32; 3],
            key: String,
        } => legal(SERVER, Sim, Write),
        SectionKvGetMany {
            key: String,
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        SectionKvSetMany {
            key: String,
            writes: Vec<([i32; 3], Option<Vec<u8>>)>,
        } => legal(SERVER, Sim, Write),
        SectionKvFind {
            section: [i32; 3],
            key: String,
        } => legal(SERVER, Sim, Read),
    }
}
