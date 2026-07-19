//! Engine-owned mob tag keys. Mob tags are open `namespace:name` strings (the
//! `petramond:` namespace is engine-reserved, mods invent `mod_id:` keys), so
//! the engine's own keys live here as consts — the same convention
//! `BlockTag`/`ItemTag` use for block and item identity — instead of literals
//! sprinkled through the sim.

/// `Bool(true)` while the mob's movement space is enclosed (penned/captive),
/// refreshed periodically by the instance tick (see `confined.rs`). AI
/// behaviors that rely on freedom of movement (e.g., herd cohesion) ignore
/// confined companions; mods may read it through the `MobTagGet` HostCall.
pub const CONFINED: &str = "petramond:confined";
