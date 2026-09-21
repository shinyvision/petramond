//! Work done every N ticks, spread over the ticks between.

/// A period in ticks. `due(now, id)` is true on one tick of every period, and
/// WHICH tick depends on `id`: a hundred machines checked "every 40 ticks"
/// each pick their own tick of the forty instead of all spiking on the same
/// one.
///
/// ```ignore
/// const CHECK: Cadence = Cadence::every(40);
/// if CHECK.due(current_tick(), machine_id) { /* ... */ }
/// ```
///
/// Pass a stable number for `id` (an entity id, a record id, a hash of a
/// position); `0` for work there is only one of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence(u64);

impl Cadence {
    /// Every `ticks` ticks; `0` reads as every tick.
    pub const fn every(ticks: u64) -> Self {
        Self(if ticks == 0 { 1 } else { ticks })
    }

    pub const fn due(self, now: u64, id: u64) -> bool {
        now % self.0 == id % self.0
    }

    pub const fn ticks(self) -> u64 {
        self.0
    }
}
