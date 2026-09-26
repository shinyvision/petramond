//! Input vocabulary for the client and the engine's client-side modding and
//! settings code: windowing-toolkit-free key identities ([`keycode`]) and the
//! key-binding engine that resolves raw device events into controls
//! ([`controls`]). The deterministic world core does not depend on it.

pub mod controls;
pub mod keycode;
