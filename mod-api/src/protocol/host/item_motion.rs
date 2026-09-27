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
    }
}
