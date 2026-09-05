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
// 32: claimed player body state — `SelfState` gains the resolved
//     `move_scale`, the barred-action set, and the per-hand held poses, which
//     the player rows carry too so every observer sees a raised guard on
//     somebody else's body, plus the rig-bone offsets that move the arm
//     holding it. Bones ride as RIG IDS (`player::BonePose`), never names: the
//     row ships for every player every tick, and the ABI's names resolve once
//     at the host call.
//     `move_scale` and the barred-action set carry only the half the recipient
//     cannot derive: the engine's own claim (status-effect speed, spectator, an
//     open menu) is folded by BOTH mirrors from state they both hold, so
//     sending it would double it. Same shape, same size — no bump.
// 33: the per-hand hand-motion claim (`motion_claims: [HandMotions; 2]`) on
//     the player rows and `SelfState` — while a motion is claimed, every
//     mirror's vanilla copy of it stands down for that hand because the
//     claimant animates it through the pose seams.
// 35: `SpatialSoundMsg::Set` (a live spatial sound retuned in place —
//     the loop rows' volume follows what the mod integrates, e.g. a cart's
//     speed) appended after `Stop`.
pub const PROTOCOL_VERSION: u16 = 35;

/// The default server port: used by "Open to LAN" and by "Connect to server"
/// addresses that don't name a `:port`.
pub const DEFAULT_PORT: u16 = 7434;
