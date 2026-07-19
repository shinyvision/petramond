//! Instance animation: the engine expression clock (walk/idle/rest selection
//! and head-look easing) plus the mod-controlled named animation layers
//! (`MobAnimSet` / `MobAnimRate` / `MobAnimSeek`).

use mod_api::{MAX_MOB_ANIM_NAME_BYTES, MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE};

use super::brain::BehaviorOutput;
use super::instance::Instance;
use super::kinematics::{approach, turn_toward};
use super::MobDef;

/// How fast the head turns toward its look target (rad/s) — deliberately slow so the
/// head pans rather than snaps.
const HEAD_TURN_RATE: f32 = 4.0;

/// Which animation a mob is playing — drives `anim_time` advance rate + reset.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum AnimKind {
    Walk,
    Idle(u8),
    Rest,
}

/// One active named model animation: its self-clocked playback state. The
/// phase is SECONDS into the authored clip (the renderer wraps looping clips
/// by their length) and is what replicates; the rate and seek target are
/// server-side control state only. While `seek` is set, the phase approaches
/// it directly at `|rate|`/s and lands EXACTLY on it (then holds at rate 0)
/// — how an oar settles back onto its authored pose.
#[derive(Clone, Debug)]
pub struct AnimLayer {
    pub name: String,
    pub phase: f32,
    pub rate: f32,
    pub seek: Option<f32>,
}

/// Advance one named animation without ever publishing non-finite or
/// unbounded control state. Host guards make this defensive in normal play;
/// keeping the invariant here also contains corrupted/internally-produced
/// state before it reaches replication.
fn step_anim_layer(layer: &mut AnimLayer, dt: f32) {
    if !layer.phase.is_finite() {
        layer.phase = 0.0;
        layer.rate = 0.0;
        layer.seek = None;
        return;
    }
    if layer.phase.abs() > MAX_MOB_ANIM_PHASE_MAGNITUDE {
        layer.phase = layer
            .phase
            .clamp(-MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_PHASE_MAGNITUDE);
        layer.rate = 0.0;
        layer.seek = None;
        return;
    }
    if !dt.is_finite()
        || dt < 0.0
        || !layer.rate.is_finite()
        || layer.rate.abs() > MAX_MOB_ANIM_RATE_MAGNITUDE
        || layer.seek.is_some_and(|target| {
            !target.is_finite() || target.abs() > MAX_MOB_ANIM_PHASE_MAGNITUDE
        })
    {
        layer.rate = 0.0;
        layer.seek = None;
        return;
    }

    let next = match layer.seek {
        Some(target) => {
            let step = layer.rate.abs() * dt;
            if !step.is_finite() {
                layer.rate = 0.0;
                layer.seek = None;
                return;
            }
            if (target - layer.phase).abs() <= step {
                layer.rate = 0.0;
                layer.seek = None;
                target
            } else {
                layer.phase + step * (target - layer.phase).signum()
            }
        }
        None => layer.phase + layer.rate * dt,
    };
    if next.is_finite() && next.abs() <= MAX_MOB_ANIM_PHASE_MAGNITUDE {
        layer.phase = next;
    } else {
        layer.rate = 0.0;
        layer.seek = None;
    }
}

impl Instance {
    /// The active named model animations, sorted by name.
    #[inline]
    pub fn active_anims(&self) -> &[AnimLayer] {
        &self.active_anims
    }

    /// Toggle one named model animation — the animation sibling of
    /// [`set_emitter_active`](Self::set_emitter_active). Activation starts
    /// the layer at phase 0, rate 1 (see [`set_anim_rate`](Self::set_anim_rate)).
    /// Returns `false` only when an activation would exceed
    /// [`super::MAX_ACTIVE_MOB_ANIMS`]. The name is NOT validated against the
    /// model (the sim does not load models); the renderer skips names the
    /// model lacks, like a disabled pack's content.
    pub(super) fn set_anim_active(&mut self, name: &str, active: bool) -> bool {
        if name.len() > MAX_MOB_ANIM_NAME_BYTES {
            return false;
        }
        match (self.anim_search(name), active) {
            (Ok(_), true) | (Err(_), false) => true,
            (Ok(at), false) => {
                self.active_anims.remove(at);
                true
            }
            (Err(at), true) => {
                if self.active_anims.len() >= super::MAX_ACTIVE_MOB_ANIMS {
                    return false;
                }
                self.active_anims.insert(
                    at,
                    AnimLayer {
                        name: name.to_owned(),
                        phase: 0.0,
                        rate: 1.0,
                        seek: None,
                    },
                );
                true
            }
        }
    }

    /// Set an active layer's playback rate (phase advance per second): `0`
    /// freezes it mid-stroke, negative reverses. Cancels an in-flight seek.
    /// `false` when the anim isn't active.
    pub(super) fn set_anim_rate(&mut self, name: &str, rate: f32) -> bool {
        if name.len() > MAX_MOB_ANIM_NAME_BYTES
            || !rate.is_finite()
            || rate.abs() > MAX_MOB_ANIM_RATE_MAGNITUDE
        {
            return false;
        }
        let Some(layer) = self.active_anim_mut(name) else {
            return false;
        };
        layer.rate = rate;
        layer.seek = None;
        true
    }

    /// Seek an active layer's phase to the absolute `target` at `|rate|`/s
    /// (see [`AnimLayer`]). `false` when the anim isn't active.
    pub(super) fn set_anim_seek(&mut self, name: &str, target: f32, rate: f32) -> bool {
        if name.len() > MAX_MOB_ANIM_NAME_BYTES
            || !target.is_finite()
            || target.abs() > MAX_MOB_ANIM_PHASE_MAGNITUDE
            || !rate.is_finite()
            || rate.abs() > MAX_MOB_ANIM_RATE_MAGNITUDE
        {
            return false;
        }
        let Some(layer) = self.active_anim_mut(name) else {
            return false;
        };
        layer.rate = rate.abs();
        layer.seek = Some(target);
        true
    }

    /// Authoritative state of one active named animation.
    pub(super) fn anim_state(&self, name: &str) -> Option<&AnimLayer> {
        if name.len() > MAX_MOB_ANIM_NAME_BYTES {
            return None;
        }
        self.anim_search(name).ok().map(|at| &self.active_anims[at])
    }

    /// Position of one named layer in the sorted `active_anims` (`Ok` =
    /// active at that index, `Err` = the insertion point).
    fn anim_search(&self, name: &str) -> Result<usize, usize> {
        self.active_anims
            .binary_search_by(|a| a.name.as_str().cmp(name))
    }

    /// The active layer named `name`, if any.
    fn active_anim_mut(&mut self, name: &str) -> Option<&mut AnimLayer> {
        let at = self.anim_search(name).ok()?;
        Some(&mut self.active_anims[at])
    }

    /// Apply the tick's expressive decision: choose + advance the active animation
    /// (walk while moving, an `idle_*` if one was requested, else the neutral rest
    /// pose), and ease the head toward the head-look target (recentring when there's
    /// none — e.g. while walking).
    pub(super) fn apply_expression(&mut self, dt: f32, d: &MobDef, decision: &BehaviorOutput) {
        // An idle animation only plays while the mob isn't walking.
        self.idle_anim = if self.moving {
            None
        } else {
            decision.idle_anim
        };

        // Pick the active animation; reset its phase whenever it changes.
        let kind = if self.moving {
            AnimKind::Walk
        } else if let Some(i) = self.idle_anim {
            AnimKind::Idle(i)
        } else {
            AnimKind::Rest
        };
        if kind != self.anim_kind {
            self.anim_kind = kind;
            self.anim_time = 0.0;
            self.prev_anim_time = 0.0;
        }
        // Advance the active animation: walk at the species' rate, idle at its
        // natural rate, rest frozen (the renderer shows the static rest pose).
        // Named mod layers do NOT ride this clock — each advances its own
        // phase at its own mod-set rate below, so one layer can pause
        // mid-stroke while another plays.
        match kind {
            AnimKind::Walk => self.anim_time += d.walk_anim_rate * dt,
            AnimKind::Idle(_) => self.anim_time += dt,
            AnimKind::Rest => {}
        }
        for layer in &mut self.active_anims {
            step_anim_layer(layer, dt);
        }

        // Head-look: ease toward the requested orientation, or recentre when none.
        let (target_yaw, target_pitch) = match decision.head_look {
            Some(h) => (h.yaw, h.pitch),
            None => (0.0, 0.0),
        };
        let step = HEAD_TURN_RATE * dt;
        self.head_yaw = turn_toward(self.head_yaw, target_yaw, step);
        self.head_pitch = approach(self.head_pitch, target_pitch, step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mathh::{IVec3, Vec3};
    use crate::mob::{def, Mob};

    fn floor_at_zero(p: IVec3) -> bool {
        p.y < 0
    }

    fn owl_def() -> &'static MobDef {
        def(Mob::Owl)
    }

    #[test]
    fn expression_advances_walk_and_eases_the_head() {
        use super::super::brain::HeadLook;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        // A walking tick (integrate sets `moving`), then expression advances the walk.
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
            &|_| false,
        );
        owl.apply_expression(1.0 / 60.0, owl_def(), &BehaviorOutput::default());
        let a1 = owl.anim_time;
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &floor_at_zero,
            &|_| false,
        );
        owl.apply_expression(1.0 / 60.0, owl_def(), &BehaviorOutput::default());
        assert!(
            owl.anim_time > a1,
            "walk cycle keeps advancing: {a1} -> {}",
            owl.anim_time
        );

        // Head eases toward a look target, then recentres when there's none.
        let look = BehaviorOutput {
            head_look: Some(HeadLook {
                yaw: 1.0,
                pitch: 0.3,
            }),
            ..Default::default()
        };
        for _ in 0..120 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &floor_at_zero,
                &|_| false,
            );
            owl.apply_expression(1.0 / 60.0, owl_def(), &look);
        }
        assert!(
            (owl.head_yaw - 1.0).abs() < 0.05,
            "head reaches yaw target: {}",
            owl.head_yaw
        );
        assert!(
            (owl.head_pitch - 0.3).abs() < 0.05,
            "head reaches pitch target: {}",
            owl.head_pitch
        );
        for _ in 0..120 {
            owl.apply_expression(1.0 / 60.0, owl_def(), &BehaviorOutput::default());
        }
        assert!(
            owl.head_yaw.abs() < 0.05,
            "head recentres in yaw: {}",
            owl.head_yaw
        );
        assert!(
            owl.head_pitch.abs() < 0.05,
            "head recentres in pitch: {}",
            owl.head_pitch
        );
    }

    #[test]
    fn named_anim_set_is_sorted_capped_and_idempotent() {
        let names = |owl: &Instance| -> Vec<String> {
            owl.active_anims().iter().map(|l| l.name.clone()).collect()
        };
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(owl.set_anim_active("row_right", true));
        assert!(owl.set_anim_active("row_left", true));
        assert!(
            owl.set_anim_active("row_left", true),
            "re-activate is a no-op"
        );
        assert_eq!(names(&owl), ["row_left", "row_right"]);
        assert!(owl.set_anim_active("c", true));
        assert!(owl.set_anim_active("d", true));
        assert!(
            !owl.set_anim_active("e", true),
            "a fifth activation refuses (cap {})",
            crate::mob::MAX_ACTIVE_MOB_ANIMS
        );
        assert!(
            owl.set_anim_active("missing", false),
            "deactivate absent = ok"
        );
        assert!(owl.set_anim_active("row_left", false));
        assert_eq!(names(&owl), ["c", "d", "row_right"]);
    }

    #[test]
    fn named_anim_layers_self_clock_by_their_rates() {
        // Each layer's phase advances by ITS OWN rate — rate 0 freezes a
        // layer mid-stroke (an oar pauses in place, never snaps home),
        // negative reverses — independent of the walk/idle clock.
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(owl.set_anim_active("a", true));
        assert!(owl.set_anim_active("b", true));
        assert!(
            !owl.set_anim_rate("missing", 2.0),
            "a rate on an inactive anim refuses"
        );
        let step = |owl: &mut Instance| {
            owl.apply_expression(0.5, def(Mob::Owl), &Default::default());
        };
        step(&mut owl); // both at default rate 1
        let phase = |owl: &Instance, n: &str| {
            owl.active_anims()
                .iter()
                .find(|l| l.name == n)
                .unwrap()
                .phase
        };
        assert_eq!((phase(&owl, "a"), phase(&owl, "b")), (0.5, 0.5));
        assert!(owl.set_anim_rate("a", 0.0));
        assert!(owl.set_anim_rate("b", -1.0));
        step(&mut owl);
        assert_eq!(phase(&owl, "a"), 0.5, "rate 0 freezes the layer in place");
        assert_eq!(phase(&owl, "b"), 0.0, "negative rate plays in reverse");
    }

    #[test]
    fn named_anim_seek_lands_exactly_holds_and_yields_to_rate() {
        // A seek approaches its target DIRECTLY at |rate|/s, lands EXACTLY on
        // it (no overshoot, then holds at rate 0) — the settle-to-pose
        // contract an oar's gentle return depends on. A rate call cancels it.
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(owl.set_anim_active("a", true));
        assert!(
            !owl.set_anim_seek("missing", 0.0, 1.0),
            "a seek on an inactive anim refuses"
        );
        let step = |owl: &mut Instance| {
            owl.apply_expression(0.5, def(Mob::Owl), &Default::default());
        };
        let phase = |owl: &Instance| owl.active_anims()[0].phase;
        step(&mut owl); // free-runs to 0.5 at the default rate 1
        assert!(owl.set_anim_seek("a", 1.7, 1.0));
        step(&mut owl);
        assert_eq!(phase(&owl), 1.0, "seeking toward the target at |rate|");
        step(&mut owl);
        step(&mut owl);
        assert_eq!(
            phase(&owl),
            1.7,
            "lands EXACTLY on the target, no overshoot"
        );
        step(&mut owl);
        assert_eq!(phase(&owl), 1.7, "then holds (rate 0)");
        assert!(owl.set_anim_seek("a", 0.7, -1.0), "rate sign is ignored");
        step(&mut owl);
        assert_eq!(phase(&owl), 1.2, "seeks BACKWARD toward a lower target");
        assert!(owl.set_anim_rate("a", 1.0));
        step(&mut owl);
        assert_eq!(phase(&owl), 1.7, "a rate call cancels the seek");
    }

    #[test]
    fn named_anim_controls_are_bounded_and_phase_stepping_stays_finite() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(!owl.set_anim_active(&"a".repeat(mod_api::MAX_MOB_ANIM_NAME_BYTES + 1), true));
        assert!(owl.set_anim_active("a", true));
        assert!(!owl.set_anim_rate("a", mod_api::MAX_MOB_ANIM_RATE_MAGNITUDE * 2.0));
        assert!(!owl.set_anim_seek("a", mod_api::MAX_MOB_ANIM_PHASE_MAGNITUDE * 2.0, 1.0));

        let layer = &mut owl.active_anims[0];
        layer.phase = f32::INFINITY;
        layer.rate = 1.0;
        layer.seek = Some(2.0);
        step_anim_layer(layer, 0.05);
        assert!(layer.phase.is_finite());
        assert_eq!(layer.rate, 0.0);
        assert_eq!(layer.seek, None);

        layer.phase = mod_api::MAX_MOB_ANIM_PHASE_MAGNITUDE;
        layer.rate = mod_api::MAX_MOB_ANIM_RATE_MAGNITUDE;
        step_anim_layer(layer, 0.05);
        assert!(layer.phase.is_finite());
        assert!(layer.phase.abs() <= mod_api::MAX_MOB_ANIM_PHASE_MAGNITUDE);
        assert_eq!(layer.rate, 0.0, "a step past the phase envelope parks");
    }
}
