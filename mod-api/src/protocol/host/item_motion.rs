//! Dropped and launched item entities in motion: radius queries and impulses.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::legality::prelude::*;

host_domain! {
    /// Dropped and launched item entities in motion: radius queries and impulses.
    ItemMotionCall {
        /// Nearest live item entities within `radius`, ordered by distance then
        /// stable id. `limit` bounds the reply (at most `SIM_BATCH_MAX`); zero
        /// returns nothing. Radius must be finite and within `0..=64`.
        /// Frozen terrain is omitted. → [`HostRet::ItemEntities`](crate::HostRet::ItemEntities).
        ItemEntitiesInRadius {
            pos: [f64; 3],
            radius: f32,
            limit: u32,
        } => legal(SERVER, Sim, Read),
        /// Add world-space velocity deltas (m/s), in request order, to live
        /// item entities. At most `SIM_BATCH_MAX` entries. A missing, lodged,
        /// pickup-reserved or terrain-frozen entity answers false; a resulting
        /// velocity outside the collision sweep bound also answers false.
        /// Invalid non-finite deltas reject the whole call before any mutation.
        /// Motion kind, stack, age and ownership are preserved. → [`HostRet::Bools`](crate::HostRet::Bools).
        ItemImpulses {
            impulses: Vec<(u64, [f32; 3])>,
        } => legal(SERVER, Sim, Write),
    }
}
