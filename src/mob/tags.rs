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

/// `Float` current health. Seeded at spawn from the species row's own
/// `tags` (`mobs.json` — the loader requires it on every row), decreased by
/// the damage pipeline, and persisted with the mob: a wounded sheep reloads
/// wounded. The row's spawn value doubles as the species maximum
/// ([`crate::mob::MobDef::spawn_health`]).
pub const HEALTH: &str = "petramond:health";

/// `Int` game ticks of coat regrowth remaining after a shear. Present and
/// positive = shorn (coat cubes hidden, can't be shorn again); it counts
/// down on the instance tick and is REMOVED at zero, so a fully coated mob
/// carries no tag at all. Persisted — a shorn sheep must not reload with
/// its wool back.
pub const SHEAR_REGROW: &str = "petramond:shear_regrow";
