//! Multiplayer networking: wire protocol types, server address parsing, and
//! registry id remapping.
//!
//! Protocol messages are plain Rust values; SERIALIZATION IS A TRANSPORT
//! CONCERN. The in-process (singleplayer / listen-host) connection passes them
//! over channels untouched — `Arc<[u8]>` section buffers are refcount bumps —
//! while the TCP transport encodes length-prefixed postcard frames on
//! its own reader/writer threads.

pub mod address;
pub mod connection;
pub mod framing;
pub mod handshake;
pub mod protocol;
pub mod remap;

/// Bumped on ANY wire-incompatible change. Checked first in the handshake —
/// nothing else is parseable across a mismatch.
// 19: menu drag/drop actions carry ordered logical slot identities.
// 21: cursor throws are one `ThrowCursor { amount }` action.
// 27: block light is a packed RGB cell (`SectionLight`), two bytes per voxel.
// 28: the chest AND furnace lost their own slot/target variants — every
//     container ships as the keyed generic `Container` target with
//     `Container(i)` slots and named gauge readings in `gui_state`, so no
//     engine content identity remains in the menu protocol.
// 31: the off-hand slot — self inventory bodies append the off-hand stack
//     after the cursor, `MenuSlotWire::OffHand`, the hovered-slot swap
//     gesture (`ClientToServer::MenuSwapOffHand`), the off-hand item +
//     acting-hand eat/use flags on the player rows.
pub const PROTOCOL_VERSION: u16 = 31;

/// The default server port: used by "Open to LAN" and by "Connect to server"
/// addresses that don't name a `:port`.
pub const DEFAULT_PORT: u16 = 7434;
