pub mod address;
pub mod blob;
pub mod connection;
pub mod framing;
pub mod handle;
pub mod handshake;
pub mod identity;
pub mod protocol;
pub mod rate;
pub mod remap;
pub mod secure;
pub mod spatial_loops;
pub mod tick_delta;

pub const PROTOCOL_VERSION: u16 = 58;

pub const DEFAULT_PORT: u16 = 7434;
