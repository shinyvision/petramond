//! Multiplayer networking: wire protocol types, server address parsing, and
//! registry id remapping.
//!
//! Protocol messages are plain Rust values; SERIALIZATION IS A TRANSPORT
//! CONCERN. The in-process (singleplayer / listen-host) connection passes them
//! over channels untouched — `Arc<[u8]>` section buffers are refcount bumps —
//! while the TCP transport (Phase E) encodes length-prefixed postcard frames on
//! its own reader/writer threads.

pub(crate) mod address;
pub(crate) mod connection;
pub(crate) mod framing;
pub(crate) mod handshake;
pub(crate) mod protocol;
pub(crate) mod remap;

/// Bumped on ANY wire-incompatible change. Checked first in the handshake —
/// nothing else is parseable across a mismatch.
// 19: menu drag/drop actions carry ordered logical slot identities.
pub(crate) const PROTOCOL_VERSION: u16 = 19;

/// The default server port: used by "Open to LAN" and by "Connect to server"
/// addresses that don't name a `:port`.
pub(crate) const DEFAULT_PORT: u16 = 7434;
