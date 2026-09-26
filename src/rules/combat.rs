//! Combat pacing shared by the server's attack dispatch and the client's
//! local swing prediction.

/// Minimum number of game ticks between two attack swings: a swing FOLLOWS
/// THROUGH before the hand may attack again, so mashing the button neither
/// lands hits every tick (which would, e.g., instakill an owl) nor cuts the
/// swing out of its own animation. 8 ticks = 0.4 s, the longest swing the
/// engine's rigs play for an unclaimed hand. A pack pacing a tool off its own
/// animation claims [`mod_api::PlayerAttribute::AttackCooldown`] to `0.0` and
/// bars the next attack itself.
pub const ATTACK_COOLDOWN_TICKS: u32 = 8;
