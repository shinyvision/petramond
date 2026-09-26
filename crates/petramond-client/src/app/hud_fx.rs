//! Short-lived HUD and hand presentation the app layers over the game: the
//! hurt shake, the hand held visible over the sleep overlay, the heart
//! wiggle, and the local rigs' graph events waiting for the next draw.
//!
//! Presentation only — decayed by render time (the heart wiggle by wall-clock
//! time) and never read by the simulation.

use crate::game::GameEvents;
use petramond_world::gui_state::HealthView;

/// How long the hurt screen/hand shake (and red edge flash) lasts. Punchy and
/// short: an unmistakable "get out of here", not a lasting wobble.
const HURT_SHAKE_SECS: f32 = 0.25;
/// How long the hand remains visible after a bed click that opens the sleep
/// overlay, giving the interact jab time to read before the sleeping view
/// takes over.
const SLEEP_INTERACT_HAND_SECS: f32 = 0.30;
/// How long a changed heart wiggles, in REAL seconds (per design: not ticks).
const HEART_WIGGLE_SECS: f64 = 0.2;

/// One heart-wiggle burst: hearts overlapping `[lo, hi)` (half-heart points —
/// the points gained by a heal or lost to a hit) shake for
/// [`HEART_WIGGLE_SECS`] of wall-clock time from `started`.
#[derive(Copy, Clone)]
struct HeartWiggle {
    lo: i32,
    hi: i32,
    started: f64,
}

/// The hurt-shake offsets for one frame: camera look jitter (radians), a hand
/// screen offset (NDC), and the red edge-vignette strength.
pub(super) struct HurtShake {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) hand: [f32; 2],
    /// Red edge-vignette strength `[0, 1]` (linear envelope — it should linger
    /// a touch longer than the motion).
    pub(super) flash: f32,
}

#[derive(Default)]
pub(super) struct HudFx {
    /// Seconds left of the hurt screen/hand shake.
    hurt_shake_t: f32,
    /// Seconds left to keep the hand visible over the sleep overlay after the
    /// bed interaction jab starts.
    sleep_hand_t: f32,
    /// The HUD health drawn last frame, for change detection. `None` while
    /// no bar is drawn (shell/spectator), so re-entering never wiggles from a
    /// stale comparison.
    prev_heart_health: Option<i32>,
    /// The active heart-wiggle burst.
    heart_wiggle: Option<HeartWiggle>,
    /// Graph events fired on the local player's rigs since the last render
    /// (the engine's own gestures and mod fires), so a gesture begun on an
    /// un-drawn update isn't lost before the next draw. Taken by the render
    /// and by NOTHING else: any other consumer of these one-shot edges keeps
    /// its own latch, because a shared latch is whoever-eats-first.
    hand_events: Vec<(petramond::player::RigId, u16)>,
}

impl HudFx {
    /// The local player was hurt: start the shake.
    pub(super) fn latch_hurt(&mut self) {
        self.hurt_shake_t = HURT_SHAKE_SECS;
    }

    /// Latch a frame's hand triggers: a bed click keeps the hand up over the
    /// sleep overlay, and the engine's gestures resolve to every local rig's
    /// graph events — the viewmodel's and the body's — through the same lane
    /// a mod's fired events arrive on.
    pub(super) fn latch_hand_triggers(&mut self, events: &GameEvents) {
        if events.bed_interacted {
            self.sleep_hand_t = SLEEP_INTERACT_HAND_SECS;
        }
        for (hand, kind) in events.one_shots() {
            self.hand_events
                .extend(petramond::player::one_shot::fired(hand, kind));
        }
        self.hand_events.extend_from_slice(&events.animator_events);
    }

    /// A session began or ended: nothing latched for the old one carries over.
    pub(super) fn reset_session(&mut self) {
        self.hand_events.clear();
        self.sleep_hand_t = 0.0;
    }

    /// No session, no health bar: a fresh world must never wiggle off a
    /// comparison against the previous session's last health.
    pub(super) fn forget_health(&mut self) {
        self.prev_heart_health = None;
        self.heart_wiggle = None;
    }

    /// Decay the timers by one render's `dt`.
    pub(super) fn advance(&mut self, dt: f32) {
        self.sleep_hand_t = (self.sleep_hand_t - dt).max(0.0);
        self.hurt_shake_t = (self.hurt_shake_t - dt).max(0.0);
    }

    /// Whether the hand is still held up over the sleep overlay.
    pub(super) fn sleep_hand_visible(&self) -> bool {
        self.sleep_hand_t > 0.0
    }

    /// Seconds of hurt shake left (the first-person animator reads it).
    pub(super) fn hurt_remaining(&self) -> f32 {
        self.hurt_shake_t
    }

    /// This frame's hurt-shake offsets. Two incommensurate frequencies so the
    /// motion reads as a tremble, not a metronome; the squared envelope
    /// front-loads the kick and dies smoothly.
    pub(super) fn shake(&self, now: f64) -> HurtShake {
        if self.hurt_shake_t <= 0.0 {
            return HurtShake {
                yaw: 0.0,
                pitch: 0.0,
                hand: [0.0, 0.0],
                flash: 0.0,
            };
        }
        let envelope = (self.hurt_shake_t / HURT_SHAKE_SECS).clamp(0.0, 1.0);
        let amp = envelope * envelope;
        let t = now as f32;
        let (a, b) = ((t * 71.0).sin(), (t * 53.0).cos());
        HurtShake {
            yaw: 0.011 * amp * a,
            pitch: 0.008 * amp * b,
            hand: [0.032 * amp * b, 0.026 * amp * a],
            flash: envelope,
        }
    }

    /// Take the local rigs' graph events for this draw.
    pub(super) fn take_hand_events(&mut self) -> Vec<(petramond::player::RigId, u16)> {
        std::mem::take(&mut self.hand_events)
    }

    /// Heart-wiggle bookkeeping for this frame's HUD `health`: ANY change — a
    /// regen heal, fall damage, a mob hit — starts a wall-clock wiggle on
    /// exactly the hearts whose half-heart points changed. Returns this
    /// frame's `(lo, hi, seconds into the burst)`, or `None` when nothing
    /// wiggles.
    pub(super) fn heart_wiggle(
        &mut self,
        health: Option<HealthView>,
        now: f64,
    ) -> Option<(i32, i32, f32)> {
        let current = health.map(|h| h.current);
        // Both sides must exist: entering/leaving spectator (or the bar first
        // appearing at world join) is not a heal.
        if let (Some(prev), Some(cur)) = (self.prev_heart_health, current) {
            if cur != prev {
                self.heart_wiggle = Some(HeartWiggle {
                    lo: cur.min(prev),
                    hi: cur.max(prev),
                    started: now,
                });
            }
        }
        self.prev_heart_health = current;
        let w = self.heart_wiggle?;
        let t = now - w.started;
        if t >= HEART_WIGGLE_SECS {
            self.heart_wiggle = None;
            return None;
        }
        Some((w.lo, w.hi, t as f32))
    }

    #[cfg(test)]
    pub(super) fn hand_events(&self) -> &[(petramond::player::RigId, u16)] {
        &self.hand_events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(current: i32) -> Option<HealthView> {
        Some(HealthView { current, max: 20 })
    }

    #[test]
    fn a_health_change_wiggles_the_changed_hearts_for_a_moment() {
        let mut fx = HudFx::default();
        assert_eq!(fx.heart_wiggle(health(20), 0.0), None, "first sight is not a change");
        assert_eq!(fx.heart_wiggle(health(17), 1.0), Some((17, 20, 0.0)));
        assert_eq!(fx.heart_wiggle(health(17), 1.1).map(|w| (w.0, w.1)), Some((17, 20)));
        assert_eq!(fx.heart_wiggle(health(17), 1.3), None, "the burst ends");
    }

    #[test]
    fn a_hidden_bar_breaks_the_comparison() {
        let mut fx = HudFx::default();
        fx.heart_wiggle(health(20), 0.0);
        fx.heart_wiggle(None, 0.5);
        assert_eq!(fx.heart_wiggle(health(10), 1.0), None);
    }

    #[test]
    fn the_hurt_shake_decays_to_rest() {
        let mut fx = HudFx::default();
        assert_eq!(fx.shake(0.3).flash, 0.0);
        fx.latch_hurt();
        assert_eq!(fx.shake(0.3).flash, 1.0);
        fx.advance(HURT_SHAKE_SECS);
        let rest = fx.shake(0.3);
        assert_eq!((rest.yaw, rest.pitch, rest.flash), (0.0, 0.0, 0.0));
    }
}
