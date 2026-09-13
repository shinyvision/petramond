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

/// A damage request's stake in its victim's immunity window — the one piece
/// of damage composition every victim kind shares. The mob pipeline spells it
/// as the presence or absence of its `petramond:immunity` component; the
/// player funnel, whose other consequences are engine-fixed, composes just
/// this.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Immunity {
    /// An ordinary hit: rejected while the victim's window is active, and
    /// opens a `ticks`-long window on a real health decrease.
    Window { ticks: u32 },
    /// Damage on its own authored clock (fluid contact, condition pulses):
    /// neither blocked by an active window nor opening one, so it never
    /// shields the victim from a real hit.
    Exempt,
}

impl Immunity {
    /// The ordinary player hit: the engine-fixed window.
    pub const PLAYER: Immunity = Immunity::Window {
        ticks: PLAYER_DAMAGE_IFRAME_TICKS,
    };

    /// Whether `timer`'s current state rejects a request composed with this.
    #[inline]
    pub fn blocks(self, timer: &DamageImmunity) -> bool {
        matches!(self, Immunity::Window { .. }) && timer.is_active()
    }

    /// The window this composition opens after a real health decrease.
    #[inline]
    pub fn grant(self, timer: &mut DamageImmunity) {
        if let Immunity::Window { ticks } = self {
            timer.grant_for(ticks);
        }
    }
}

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
