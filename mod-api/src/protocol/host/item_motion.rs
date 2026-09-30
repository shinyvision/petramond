use crate::legality::prelude::*;

host_domain! {
    ItemMotionCall {
        ItemEntitiesInRadius {
            pos: [f64; 3],
            radius: f32,
            limit: u32,
        } => legal(SERVER, Sim, Read),
        ItemImpulses {
            impulses: Vec<(u64, [f32; 3])>,
        } => legal(SERVER, Sim, Write),
        /// Steers a flight through ordinary swept physics. Renew every tick; an expired
        /// controller releases the item. `None` immediately releases it as a loose drop.
        SteerItem {
            entity: u64,
            vel: Option<[f32; 3]>,
        } => legal(SERVER, Sim, Write),
        /// Removes one live item entity, returning its complete stack exactly once.
        TakeItemEntity {
            entity: u64,
        } => legal(SERVER, Sim, Write),
    }
}
