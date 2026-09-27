#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence(u64);

impl Cadence {
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
