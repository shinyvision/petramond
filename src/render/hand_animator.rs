//! Held-item swing STATE MACHINE.
//!
//! Advances the punch animation phase each frame, turning the sim's per-frame
//! [`HeldItemFrame`] intent (mining / instant-break / place) into the flat
//! [`HeldItemView`] that the stateless hand geometry builders in [`super::hand`]
//! consume. This owns the timing — the looping mining sawtooth, the one-shot
//! break/place jab, and the place jab's reduced [`HeldItemView::swing_scale`]
//! amplitude — and nothing about geometry or pose. The renderer owns one for
//! the first-person hand / local third-person body; each REMOTE player owns
//! one too (`game/remote_players.rs`), fed from replicated flags, so every
//! view animates from the same triggers.

use super::{HeldItemFrame, HeldItemView};

/// Mining-punch swings per second. Drives the looping hand swing phase while the
/// sim reports active mining, and the one-shot break/place jab speed.
const HAND_SWING_HZ: f32 = 4.2;

/// Amplitude of the place jab relative to a full mining punch. Placing reuses the
/// punch motion at this reduced strength so it reads as "similar but softer".
const PLACE_SWING_SCALE: f32 = 0.62;
// A place jab must be softer than a full mining punch — guard at compile time.
const _: () = assert!(PLACE_SWING_SCALE < 1.0);

/// Bites per second while eating — the nibble rhythm layered over the
/// mouth-carry pose (see [`HeldItemView::eat_bob`]).
const EAT_CHEW_HZ: f32 = 4.6;
/// Seconds for the held food to make its INITIAL raise when an eat starts…
const EAT_BLEND_IN_S: f32 = 0.14;
/// …and to drop back down when it ends (finish or abort) — slightly quicker
/// so a cancelled bite snaps back responsively without popping.
const EAT_BLEND_OUT_S: f32 = 0.10;
/// Smoothing window for the progress-driven approach (`eat_near`): sim
/// progress steps at 20 TPS; easing over this many seconds hides the
/// stair-steps without lagging the 3-second drift noticeably.
const EAT_NEAR_EASE_S: f32 = 0.09;

#[derive(Copy, Clone, Debug)]
pub(crate) struct HeldItemAnimator {
    swing_t: f32,
    swing_finishing: bool,
    /// Amplitude of the swing currently in flight (see [`HeldItemView::swing_scale`]).
    swing_scale: f32,
    /// 0..1 mouth-carry blend (see [`HeldItemView::eat`]), eased toward 1 while
    /// the sim reports an eat and back to 0 after.
    eat_blend: f32,
    /// Smoothed copy of the sim's eat progress (see [`HeldItemView::eat_near`]):
    /// the slow toward-the-camera approach while the food sits at the mouth.
    eat_near: f32,
    /// Nibble oscillator phase, advanced only while eating.
    eat_phase: f32,
}

impl Default for HeldItemAnimator {
    fn default() -> Self {
        Self {
            swing_t: 0.0,
            swing_finishing: false,
            swing_scale: 1.0,
            eat_blend: 0.0,
            eat_near: 0.0,
            eat_phase: 0.0,
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

        // The EAT pose rides its own channels (mouth carry + nibble), never the
        // punch: swinging the food around does not read as eating. The blend
        // carries the item to its mouth SPOT quickly (start/finish/abort all
        // glide); `eat_near` then tracks the sim's progress so the food, while
        // wiggling in place, slowly closes the remaining DEPTH toward the
        // camera over the whole eat.
        if let Some(progress) = frame.eating {
            self.eat_blend = (self.eat_blend + dt / EAT_BLEND_IN_S).min(1.0);
            self.eat_phase = (self.eat_phase + dt * EAT_CHEW_HZ).fract();
            let target = progress.clamp(0.0, 1.0);
            // Never retreat: the food only ever approaches (a new eat starts
            // from 0 anyway, via the reset below).
            let eased = self.eat_near + (target - self.eat_near) * (dt / EAT_NEAR_EASE_S).min(1.0);
            self.eat_near = eased.max(self.eat_near).min(1.0);
        } else {
            self.eat_blend = (self.eat_blend - dt / EAT_BLEND_OUT_S).max(0.0);
            if self.eat_blend == 0.0 {
                self.eat_phase = 0.0;
                self.eat_near = 0.0;
            }
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

        // Smoothstep the blend so the raise/drop settle gently at both ends;
        // the nibble is a plain sine — its amplitude is already gated by `eat`
        // at the consumer, as is the `eat_near` approach.
        let e = self.eat_blend * self.eat_blend * (3.0 - 2.0 * self.eat_blend);
        HeldItemView {
            item: frame.item,
            block_state: frame.block_state,
            swing: self.swing_t,
            swing_scale: self.swing_scale,
            eat: e,
            eat_bob: (self.eat_phase * std::f32::consts::TAU).sin(),
            eat_near: self.eat_near,
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
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
            dt: 1.0 / 60.0,
        });
        assert!(
            view.swing > 0.5,
            "stopping mining should finish the swing forward, not rewind it"
        );

        let settled = anim.update(HeldItemFrame {
            item: None,
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
            dt: 0.5 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_plays_one_swing_for_instant_break_from_rest() {
        let mut anim = HeldItemAnimator::default();

        let started = anim.update(HeldItemFrame {
            item: None,
            block_state: Default::default(),
            mining: false,
            broke_block: true,
            placed: false,
            swung: false,
            eating: None,
            dt: 0.0,
        });
        assert_eq!(
            started.swing, 0.0,
            "zero-dt break event can begin at the rest pose"
        );

        let moving = anim.update(HeldItemFrame {
            item: None,
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
            dt: 1.0 / 60.0,
        });
        assert!(
            moving.swing > 0.0,
            "instant block break should keep animating after the break frame"
        );

        let settled = anim.update(HeldItemFrame {
            item: None,
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
            dt: 1.0 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_plays_one_full_swing_for_an_attack() {
        let mut anim = HeldItemAnimator::default();
        let started = anim.update(HeldItemFrame {
            item: None,
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: true,
            eating: None,
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
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
            dt: 1.0 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0, "the attack swing completes");
    }

    #[test]
    fn animator_turns_place_event_into_one_softer_swing() {
        let mut anim = HeldItemAnimator::default();
        let placed = anim.update(HeldItemFrame {
            item: Some(ItemType::Dirt),
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: true,
            swung: false,
            eating: None,
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
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
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
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: true,
            swung: false,
            eating: None,
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
    fn animator_eat_rides_its_own_channels_not_the_swing() {
        let mut anim = HeldItemAnimator::default();
        let eat_frame = |dt: f32, progress: f32| HeldItemFrame {
            item: Some(ItemType::Stone),
            block_state: Default::default(),
            mining: false,
            broke_block: false,
            placed: false,
            swung: false,
            eating: Some(progress),
            dt,
        };

        // The raise eases in (not a snap), never plays the punch, and settles
        // at the FULL mouth spot regardless of progress — the progress drives
        // only the toward-the-camera approach, not the screen carry.
        let first = anim.update(eat_frame(1.0 / 60.0, 0.0));
        assert!(first.eat > 0.0 && first.eat < 1.0, "carry eases in");
        assert_eq!(first.swing, 0.0, "eating never plays the punch channel");
        let raised = anim.update(eat_frame(1.0, 0.0));
        assert_eq!(raised.eat, 1.0, "the raise settles at the mouth spot");
        assert!(
            raised.eat_near < 0.05,
            "no camera approach yet at progress 0, got {}",
            raised.eat_near
        );

        // The approach tracks the sim's progress monotonically, reaching the
        // camera-nearest seat only at the end of the eat.
        let mid = anim.update(eat_frame(1.0, 0.5));
        assert!(
            (mid.eat_near - 0.5).abs() < 1e-3,
            "half-eaten food is halfway through its approach, got {}",
            mid.eat_near
        );
        let done = anim.update(eat_frame(1.0, 1.0));
        assert!(
            (done.eat_near - 1.0).abs() < 1e-3,
            "the last bite happens nearest the camera, got {}",
            done.eat_near
        );

        // The nibble oscillates sign over a bite period.
        let a = anim.update(eat_frame(0.5 / EAT_CHEW_HZ, 1.0)).eat_bob;
        let b = anim.update(eat_frame(0.5 / EAT_CHEW_HZ, 1.0)).eat_bob;
        assert!(
            a.signum() != b.signum() || (a - b).abs() > 0.5,
            "the bite rhythm oscillates: {a} vs {b}"
        );

        // Ending the eat eases the carry back out to rest.
        let releasing = anim.update(HeldItemFrame {
            eating: None,
            ..eat_frame(1.0 / 60.0, 1.0)
        });
        assert!(releasing.eat < 1.0, "release starts easing out");
        let rested = anim.update(HeldItemFrame {
            eating: None,
            ..eat_frame(1.0, 1.0)
        });
        assert_eq!(rested.eat, 0.0, "the carry returns fully to rest");
    }

    #[test]
    fn animator_mining_punch_is_full_strength() {
        let mut anim = HeldItemAnimator::default();
        let view = anim.update(HeldItemFrame {
            item: None,
            block_state: Default::default(),
            mining: true,
            broke_block: false,
            placed: false,
            swung: false,
            eating: None,
            dt: 1.0 / 60.0,
        });
        assert_eq!(view.swing_scale, 1.0, "mining is the full-strength punch");
    }
}
