//! Deterministic gameplay rules shared by the authoritative server and the
//! client's prediction/presentation mirrors.
//!
//! Everything here is a pure fact or function of replicated state: both sides
//! run the SAME code instead of the client keeping a copy of server-owned
//! logic. Nothing in this module reads a server session or client-only state —
//! the server (`crate::server`) and the client crate both import from here,
//! and neither imports the other.

pub mod breaking;
pub mod combat;
pub mod daynight;
pub mod interact;
pub mod placement;
