//! Shared engine-owned damage immunity.
//!
//! Every damageable entity carries one of these timers. A real health loss
//! grants the window; every fixed game tick advances it once, before any
//! damage source can run. This makes the immunity global across attack, fall,
//! environment, and mod damage without coupling it to any one source.

/// One second of player damage immunity at the fixed 20 TPS simulation rate.
pub const PLAYER_DAMAGE_IFRAME_TICKS: u32 = 20;
/// Mob immunity is tuned separately because its combat feel is different.
pub const MOB_DAMAGE_IFRAME_TICKS: u32 = 10;

#[derive(Clone, Debug, Default)]
pub struct DamageImmunity {
    remaining: u32,
}

impl DamageImmunity {
    #[inline]
    pub fn is_active(&self) -> bool {
        self.remaining > 0
    }

    #[inline]
    pub fn grant_for(&mut self, ticks: u32) {
        self.remaining = ticks;
    }

    #[inline]
    pub fn tick(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }

    #[inline]
    pub fn clear(&mut self) {
        self.remaining = 0;
    }
}
