//! The bodies whose footsteps the client may sound this frame: an audio
//! input produced beside the body animation, never part of the render
//! presentation contract.

/// One body whose FOOTSTEPS the client may sound this frame — the local player
/// and every visible remote, together, so a step is heard at whoever took it.
///
/// Built by the presentation gather because deciding it needs the world: the
/// sound is the block UNDER the feet, and only presentation has the replica.
/// The cadence itself is the client audio's (see `tick_footsteps`), like the
/// mob idle schedule.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FootstepSource {
    /// Stable cadence key: `0` is the local player, a remote is `1 + its
    /// PlayerId`. Ids are only compared, never sent.
    pub id: u64,
    /// Feet centre — where the sound plays, so a remote's steps arrive from
    /// their body and attenuate with distance like any other world sound.
    pub pos: petramond_math::world_pos::WorldPos,
    /// The block being walked on, or `None` when this body is not making
    /// footsteps at all: standing still, SNEAKING, airborne (the cell below is
    /// air), seated, asleep, or over an unloaded cell. The audio never
    /// re-decides this.
    pub ground: Option<petramond_world::block::Block>,
    /// Moving at a sprint — the gait the audio picks the step interval from.
    ///
    /// Derived from the body's ACTUAL horizontal speed on both sides rather
    /// than from a sprint key: a key held while the body is blocked, wading, or
    /// climbing must not quicken the cadence, and speed needs no new
    /// replication (a remote's velocity already ships for its walk blend).
    pub sprinting: bool,
}
