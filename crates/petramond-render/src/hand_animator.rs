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

use petramond_world::item::ItemType;

use super::vanilla_swing::vanilla_swing;
use super::{HeldItemFrame, HeldItemView, HeldPose, POSE_EASE_RATE};

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

/// How fast the hand's sway chases the camera's, per second.
///
/// This single number IS the "not perfectly in sync" — a first-order lag, not a
/// fixed phase offset, because it also has to behave at the START and END of a
/// walk (an offset would have the hand swaying before the first step and after
/// the last). At a walk the camera's sway runs near 9.5 rad/s, so this rate
/// puts the hand about 50 degrees behind at roughly two thirds amplitude:
/// clearly trailing, never obviously wrong. Raising it walks the hand back into
/// lockstep with the camera, which is the look this exists to avoid.
const HAND_BOB_CHASE_RATE: f32 = 8.0;

/// Peak hand sway in view units at full stride, side and up. The hand sits much
/// nearer the eye than anything in the world, so it carries a larger motion
/// than the camera's own without reading as bigger.
const HAND_BOB_SWAY: f32 = 0.060;
const HAND_BOB_RISE: f32 = 0.050;
/// How much of the hands' inertial offset the EAT pose damps at full blend:
/// food carried to the mouth is held steady, not thrown about.
const EAT_MOTION_DAMPING: f32 = 0.7;

#[derive(Copy, Clone, Debug)]
pub struct HeldItemAnimator {
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
    /// The eased held pose (see [`HeldItemView::pose`]): lags `pose_target`
    /// the way the bob lags the camera.
    pose: HeldPose,
    /// Which item the eased pose belongs to. A pose is state ABOUT AN ITEM,
    /// so a hand that changed item must not glide the old item's offset onto
    /// the new one — see the reset in [`HeldItemAnimator::update`].
    posed_item: Option<ItemType>,
    /// Which item the in-progress EAT belongs to, latched when it starts.
    ///
    /// The eat flag is REPLICATED and the hotbar swap that aborts it is local,
    /// so for a batch the sim still says "eating" about food this hand is no
    /// longer holding. The item is the authority in that window, not the flag.
    eat_item: Option<ItemType>,
    /// Whether the sim reported an eat last frame — the edge the latch rides.
    was_eating: bool,
    /// The hand's lagging copy of the camera's walk sway, normalized.
    bob: [f32; 2],
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
            pose: HeldPose::default(),
            posed_item: None,
            eat_item: None,
            was_eating: false,
            bob: [0.0, 0.0],
        }
    }
}

impl HeldItemAnimator {
    pub fn update(&mut self, frame: HeldItemFrame) -> HeldItemView {
        let dt = frame.dt.max(0.0);

        // A NEW ITEM starts at its own authored hold, with no trace of the
        // outgoing one's animation. Everything positional here is state ABOUT
        // AN ITEM: carried across a hotbar switch it draws the incoming item
        // where the outgoing one was and animates it into place, which reads
        // as the new item rising into position rather than simply being held.
        //
        // The EAT channels need it as much as the pose does — swapping out
        // mid-meal glided a pickaxe down from the mouth — and they cannot just
        // ease out, because the eat that owned them is already over.
        if frame.item != self.posed_item {
            self.posed_item = frame.item;
            self.pose = HeldPose::default();
            self.eat_blend = 0.0;
            self.eat_near = 0.0;
            self.eat_phase = 0.0;
        }
        // The hand CHASES the camera's sway instead of wearing it: the arm has
        // mass, and a hand locked to the camera reads as painted on the screen.
        let chase = 1.0 - (-HAND_BOB_CHASE_RATE * dt).exp();
        for (have, want) in self.bob.iter_mut().zip(frame.bob) {
            *have += (want - *have) * chase;
        }

        // A placement plays one softer swing — the same punch motion as mining,
        // at reduced amplitude. Restart the phase so the jab reads cleanly even
        // mid-recovery; when the placement empties the hand it carries straight
        // onto the bare arm, since both placements read this same `swing` phase.
        // A claimed JAB never starts (the claimant animates the use gesture
        // itself); one already in flight finishes home below.
        if frame.placed && !frame.jab_claim {
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
        // An eat belongs to the item it STARTED on (see `eat_item`).
        if frame.eating.is_some() && !self.was_eating {
            self.eat_item = frame.item;
        }
        self.was_eating = frame.eating.is_some();
        let eating = frame.eating.filter(|_| self.eat_item == frame.item);
        if let Some(progress) = eating {
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

        // From there the pose CHASES its target like the bob chases the
        // camera — the lag is what turns a replicated publisher's stair-steps
        // into a glide. `None` eases back to the item's authored hold.
        self.pose.ease_toward(
            &frame.pose_target.unwrap_or_default(),
            1.0 - (-POSE_EASE_RATE * dt).exp(),
        );

        // The swing state machine. Each claimed MOTION plays nothing of the
        // engine's own: under a swing claim the mining loop is silenced (the
        // level is ignored) and the full-strength break/attack punches never
        // start, because the claimant animates those through the pose seam
        // and the vanilla motion layered under its curve would be two swings
        // fighting one another; a jab claim stops `placed` latching above
        // the same way. Unclaimed motions keep their engine defaults — a
        // swing-only claimant's hand still pops on a placement. An arc
        // already in flight when a claim lands finishes home rather than
        // snapping. The swing FACTS keep publishing exactly as before; only
        // the engine's own copy of a claimed motion stands down.
        if frame.swing_claim {
            self.advance_one_shot(dt);
        } else if frame.mining {
            self.swing_finishing = false;
            self.swing_scale = 1.0;
            // The mining sawtooth paces on the AUTHORED work window
            // (hand_swing.json `window_mine`) — the vanilla swing rate is
            // data now, exactly like a pack's.
            self.swing_t = (self.swing_t + dt / vanilla_swing().loop_seconds).fract();
        } else {
            // A block break and an attack swing (mob hit / punch) both play a single
            // full-strength swing. They never coincide with `mining` (mining needs a
            // block under the crosshair; an attack nulls that look).
            if frame.broke_block || frame.swung {
                self.swing_finishing = true;
                self.swing_scale = 1.0;
            }
            self.advance_one_shot(dt);
        }

        // Smoothstep the eat blend so the raise/drop settle gently at both
        // ends; the nibble is a plain sine — its amplitude is already gated
        // by `eat` at the consumer, as is the `eat_near` approach. The pose
        // offset is already smoothed by its chase.
        let e = self.eat_blend * self.eat_blend * (3.0 - 2.0 * self.eat_blend);
        HeldItemView {
            item: frame.display.or(frame.item),
            hold: frame
                .item
                .map(|item| item.held_pose())
                .unwrap_or(petramond_world::item::HeldPose::DEFAULT),
            variant: frame.variant,
            block_state: frame.block_state,
            bob: [self.bob[0] * HAND_BOB_SWAY, self.bob[1] * HAND_BOB_RISE],
            motion_offset: frame
                .motion_offset
                .map(|v| v * (1.0 - e * EAT_MOTION_DAMPING)),
            swing: self.swing_t,
            swing_scale: self.swing_scale,
            eat: e,
            eat_bob: (self.eat_phase * std::f32::consts::TAU).sin(),
            eat_near: self.eat_near,
            pose: self.pose,
        }
    }

    /// Advance a ONE-SHOT swing (a jab or a punch) toward rest on the
    /// authored attack window (hand_swing.json `window_attack`); an idle
    /// hand stays at rest.
    fn advance_one_shot(&mut self, dt: f32) {
        if self.swing_finishing || self.swing_t > 0.0 {
            let next = self.swing_t + dt / vanilla_swing().one_shot_seconds;
            if next >= 1.0 {
                self.swing_t = 0.0;
                self.swing_finishing = false;
            } else {
                self.swing_t = next;
            }
        }
    }
}

#[cfg(test)]
mod tests;
