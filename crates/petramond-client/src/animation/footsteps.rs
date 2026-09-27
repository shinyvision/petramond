#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FootstepSource {
    pub id: u64,
    pub pos: petramond_math::world_pos::WorldPos,
    pub ground: Option<petramond_world::block::Block>,
    /// Moving at a sprint — the gait the audio picks the step interval from.
    ///
    /// Derived from the body's ACTUAL horizontal speed on both sides rather
    /// than from a sprint key: a key held while the body is blocked, wading, or
    /// climbing must not quicken the cadence, and speed needs no new
    /// replication (a remote's velocity already ships for its walk blend).
    pub sprinting: bool,
}
