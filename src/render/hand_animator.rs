//! First-person held-item swing STATE MACHINE.
//!
//! Advances the punch animation phase each frame, turning the sim's per-frame
//! [`HeldItemFrame`] intent (mining / instant-break / place) into the flat
//! [`HeldItemView`] that the stateless hand geometry builders in [`super::hand`]
//! consume. This owns the timing — the looping mining sawtooth, the one-shot
//! break/place jab, and the place jab's reduced [`HeldItemView::swing_scale`]
//! amplitude — and nothing about geometry or pose.

use super::{HeldItemFrame, HeldItemView};

/// Mining-punch swings per second. Drives the looping hand swing phase while the
/// sim reports active mining, and the one-shot break/place jab speed.
const HAND_SWING_HZ: f32 = 4.2;

/// Amplitude of the place jab relative to a full mining punch. Placing reuses the
/// punch motion at this reduced strength so it reads as "similar but softer".
const PLACE_SWING_SCALE: f32 = 0.62;
// A place jab must be softer than a full mining punch — guard at compile time.
const _: () = assert!(PLACE_SWING_SCALE < 1.0);

#[derive(Copy, Clone, Debug)]
pub(super) struct HeldItemAnimator {
    swing_t: f32,
    swing_finishing: bool,
    /// Amplitude of the swing currently in flight (see [`HeldItemView::swing_scale`]).
    swing_scale: f32,
}

impl Default for HeldItemAnimator {
    fn default() -> Self {
        Self {
            swing_t: 0.0,
            swing_finishing: false,
            swing_scale: 1.0,
        }
    }
}

impl HeldItemAnimator {
    pub fn update(&mut self, frame: HeldItemFrame) -> HeldItemView {
        let dt = frame.dt.max(0.0);

        // A placement plays one softer swing — the same punch motion as mining,
        // at reduced amplitude. Restart the phase so the jab reads cleanly even
        // mid-recovery; when the placement empties the hand it carries straight
        // onto the bare arm, since both placements read this same `swing` phase.
        if frame.placed {
            self.swing_t = 0.0;
            self.swing_finishing = true;
            self.swing_scale = PLACE_SWING_SCALE;
        }

        if frame.mining {
            self.swing_finishing = false;
            self.swing_scale = 1.0;
            self.swing_t = (self.swing_t + dt * HAND_SWING_HZ).fract();
        } else {
            // A block break and an attack swing (mob hit / punch) both play a single
            // full-strength swing. They never coincide with `mining` (mining needs a
            // block under the crosshair; an attack nulls that look).
            if frame.broke_block || frame.swung {
                self.swing_finishing = true;
                self.swing_scale = 1.0;
            }
            if self.swing_finishing || self.swing_t > 0.0 {
                let next = self.swing_t + dt * HAND_SWING_HZ;
                if next >= 1.0 {
                    self.swing_t = 0.0;
                    self.swing_finishing = false;
                } else {
                    self.swing_t = next;
                }
            }
        }

        HeldItemView {
            item: frame.item,
            swing: self.swing_t,
            swing_scale: self.swing_scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    #[test]
    fn animator_completes_active_swing_when_mining_stops() {
        let mut anim = HeldItemAnimator {
            swing_t: 0.5,
            ..HeldItemAnimator::default()
        };
        let view = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 1.0 / 60.0,
        });
        assert!(
            view.swing > 0.5,
            "stopping mining should finish the swing forward, not rewind it"
        );

        let settled = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 0.5 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_plays_one_swing_for_instant_break_from_rest() {
        let mut anim = HeldItemAnimator::default();

        let started = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: true,
            placed: false,
            swung: false,
            dt: 0.0,
        });
        assert_eq!(
            started.swing, 0.0,
            "zero-dt break event can begin at the rest pose"
        );

        let moving = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 1.0 / 60.0,
        });
        assert!(
            moving.swing > 0.0,
            "instant block break should keep animating after the break frame"
        );

        let settled = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 1.0 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_plays_one_full_swing_for_an_attack() {
        let mut anim = HeldItemAnimator::default();
        let started = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            swung: true,
            dt: 1.0 / 60.0,
        });
        assert!(started.swing > 0.0, "an attack begins a swing");
        assert_eq!(
            started.swing_scale, 1.0,
            "an attack swings at full strength"
        );

        // It carries through and settles like any one-shot swing.
        let settled = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 1.0 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0, "the attack swing completes");
    }

    #[test]
    fn animator_turns_place_event_into_one_softer_swing() {
        let mut anim = HeldItemAnimator::default();
        let placed = anim.update(HeldItemFrame {
            item: Some(ItemType::Dirt),
            mining: false,
            broke_block: false,
            placed: true,
            swung: false,
            dt: 1.0 / 60.0,
        });
        // A place starts a one-shot swing at the reduced place amplitude (softer
        // than a mining punch — the `PLACE_SWING_SCALE < 1.0` guard is a static
        // assertion at the constant's definition).
        assert!(placed.swing > 0.0, "place should begin a swing");
        assert_eq!(placed.swing_scale, PLACE_SWING_SCALE);

        // ...which completes and returns to rest within one swing period.
        let settled = anim.update(HeldItemFrame {
            item: Some(ItemType::Dirt),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 1.0 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_place_swing_carries_onto_emptied_hand() {
        // Placing the last block empties the hand the same frame (item -> None).
        // The swing must still fire so the bare arm animates the placement.
        let mut anim = HeldItemAnimator::default();
        let view = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: true,
            swung: false,
            dt: 1.0 / 60.0,
        });
        assert_eq!(
            view.item, None,
            "hand is empty after placing the last block"
        );
        assert!(
            view.swing > 0.0,
            "the emptied hand still plays the place swing"
        );
        assert_eq!(view.swing_scale, PLACE_SWING_SCALE);
    }

    #[test]
    fn animator_mining_punch_is_full_strength() {
        let mut anim = HeldItemAnimator::default();
        let view = anim.update(HeldItemFrame {
            item: None,
            mining: true,
            broke_block: false,
            placed: false,
            swung: false,
            dt: 1.0 / 60.0,
        });
        assert_eq!(view.swing_scale, 1.0, "mining is the full-strength punch");
    }
}
