pub const PLAYER_DAMAGE_IFRAME_TICKS: u32 = 20;
pub const MOB_DAMAGE_IFRAME_TICKS: u32 = 10;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Immunity {
    Window { ticks: u32 },
    Exempt,
}

impl Immunity {
    pub const PLAYER: Immunity = Immunity::Window {
        ticks: PLAYER_DAMAGE_IFRAME_TICKS,
    };

    #[inline]
    pub fn blocks(self, timer: &DamageImmunity) -> bool {
        matches!(self, Immunity::Window { .. }) && timer.is_active()
    }

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
