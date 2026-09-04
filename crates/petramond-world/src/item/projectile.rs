//! How an item behaves once LAUNCHED (`petramond:projectile` in an item
//! row's `data`): what pulls it down, what slows it, whether it lodges in
//! what it hits. Row-owned content like `food` or `petramond:tool` — the
//! engine flies any launched item; a row states only how its own item
//! flies, and a row that states nothing gets a plain toss. Which way the
//! item's ART points while it flies is presentation, on the row's
//! `sprite_axis` field beside its held pose.

/// The `petramond:projectile` data key on an item row.
pub const PROJECTILE_DATA_KEY: &str = "petramond:projectile";

/// Downward acceleration on a launched item that states none (m/s²) — the
/// dropped-item fall, so an unconfigured launch tumbles like a thrown stack.
pub const DEFAULT_GRAVITY: f32 = 20.0;

/// One item's flight parameters.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Projectile {
    /// Downward acceleration, m/s².
    pub gravity: f32,
    /// Fraction of speed lost per second of flight (`0` = none, `1` = stops
    /// within the second): air resistance, applied as an exponential decay.
    pub drag: f32,
    /// Whether a block impact LODGES the item — it stays where it struck,
    /// heading kept, until the block goes — rather than dropping it loose.
    pub sticks: bool,
}

impl Default for Projectile {
    fn default() -> Self {
        Projectile {
            gravity: DEFAULT_GRAVITY,
            drag: 0.0,
            sticks: false,
        }
    }
}

impl Projectile {
    /// The speed multiplier `dt` seconds of drag leave: `(1 - drag)^dt`.
    #[inline]
    pub fn drag_factor(self, dt: f32) -> f32 {
        (1.0 - self.drag).max(0.0).powf(dt)
    }
}
