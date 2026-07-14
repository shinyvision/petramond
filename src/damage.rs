//! Shared engine-owned damage immunity.
//!
//! Every damageable entity carries one of these timers. A real health loss
//! grants the window; every fixed game tick advances it once, before any
//! damage source can run. This makes the immunity global across attack, fall,
//! environment, and mod damage without coupling it to any one source.

/// One second of player damage immunity at the fixed 20 TPS simulation rate.
pub(crate) const PLAYER_DAMAGE_IFRAME_TICKS: u32 = 20;
/// Mob immunity is tuned separately because its combat feel is different.
pub(crate) const MOB_DAMAGE_IFRAME_TICKS: u32 = 15;

#[derive(Clone, Debug, Default)]
pub(crate) struct DamageImmunity {
    remaining: u32,
}

impl DamageImmunity {
    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.remaining > 0
    }

    #[inline]
    pub(crate) fn grant_for(&mut self, ticks: u32) {
        self.remaining = ticks;
    }

    #[inline]
    pub(crate) fn tick(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.remaining = 0;
    }
}
