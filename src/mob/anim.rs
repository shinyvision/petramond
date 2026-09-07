//! Instance animation: the engine expression clock (walk/idle/rest selection
//! and head-look easing) plus the mod-controlled named animation layers
//! (`MobAnimSet` / `MobAnimRate` / `MobAnimSeek`).

use mod_api::{MAX_MOB_ANIM_NAME_BYTES, MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE};
use petramond_world::bbmodel::clips;

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
    pub(super) fn apply_expression(
        &mut self,
        dt: f32,
        d: &MobDef,
        named_anims: &[super::model_meta::NamedAnimMeta],
        decision: &BehaviorOutput,
    ) {
        // Model-owned ambient details keep their own clock through gait changes.
        if let Ok(at) = named_anims.binary_search_by(|m| m.name.as_str().cmp(clips::AMBIENT)) {
            let meta = &named_anims[at];
            if meta.looping
                && meta.length > 0.0
                && self.anim_state(clips::AMBIENT).is_none()
                && self.set_anim_active(clips::AMBIENT, true)
            {
                // Stagger the loop across a herd by the stable id, so
                // blinks and ear flicks never fire in lockstep.
                let phase = phase_stagger(self.id) * meta.length;
                if let Some(layer) = self.active_anim_mut(clips::AMBIENT) {
                    layer.phase = phase;
                }
            }
        }
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
        // An upward launch from a walking gait re-phases the walk clip
        // FORWARD to the next cycle boundary. A repeating launch's period
        // (a mod-authored hop) can sit near the clip's own length, and a
        // free-running clock then drifts into anti-phase and stays there for
        // whole walk legs (legs tucking at takeoff, kicking at landing).
        // Forward only, never a reset to 0: the replica interpolates raw
        // prev→curr snapshots, so a backward phase jump sweeps the clip in
        // reverse on clients for a frame at every launch. A walk leg's first
        // launch starts at phase 0 already (the kind change above just
        // reset the clock).
        if std::mem::take(&mut self.walk_launch) && kind == AnimKind::Walk {
            let walk_len = named_anims
                .binary_search_by(|m| m.name.as_str().cmp(clips::WALK))
                .ok()
                .map(|at| named_anims[at].length)
                .filter(|len| *len > 1e-4);
            if let Some(len) = walk_len {
                let residue = self.anim_time.rem_euclid(len);
                if residue > 1e-4 {
                    self.anim_time += len - residue;
                }
            }
        }
        // Advance the active animation: walk at the species' rate, idle at its
        // natural rate, rest frozen (the renderer shows the static rest pose).
        // Named mod layers do NOT ride this clock — each advances its own
        // phase at its own mod-set rate below, so one layer can pause
        // mid-stroke while another plays.
        match kind {
            AnimKind::Walk => self.anim_time += d.walk_anim_rate * self.walk_speed_scale * dt,
            AnimKind::Idle(_) => self.anim_time += dt,
            AnimKind::Rest => {}
        }
        for layer in &mut self.active_anims {
            step_anim_layer(layer, dt);
        }
        // A ONE-SHOT layer that has played through retires itself: activation
        // is fire-and-forget for the mod (the sheep's `eat` bite finishes on
        // its own, freeing the layer slot and releasing head-look). Only
        // plain forward playback completes — a mod-driven seek or rate hold
        // (the boat oar freeze) is a deliberate pose and never expires, and a
        // looping clip plays until deactivated. A name the model doesn't
        // carry has no meta and stays (it draws nothing; the mod's business).
        self.active_anims.retain(|layer| {
            let finished = layer.seek.is_none()
                && layer.rate > 0.0
                && named_anims
                    .binary_search_by(|m| m.name.as_str().cmp(&layer.name))
                    .ok()
                    .map(|at| &named_anims[at])
                    .is_some_and(|m| !m.looping && m.length > 0.0 && layer.phase >= m.length);
            !finished
        });
        if let Some(name) = &decision.animation {
            self.set_anim_active(name, true);
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

/// A per-mob fraction in `[0, 1)` from the stable id: the Fibonacci-hash
/// multiplier spreads consecutive ids evenly, and the top 24 bits of the
/// product make a clean float mantissa.
fn phase_stagger(id: u64) -> f32 {
    const FIBONACCI_HASH: u64 = 0x9e37_79b9_7f4a_7c15;
    let hashed = id.wrapping_mul(FIBONACCI_HASH);
    (hashed >> 40) as f32 / (1u32 << 24) as f32
}

/// Whether `name` may name a clip across the AI/ABI seams: nonempty and within
/// the replicated name bound. The model is not consulted — the sim does not
/// load models; the renderer skips names the model lacks.
pub fn valid_clip_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_MOB_ANIM_NAME_BYTES
}

#[cfg(test)]
mod tests;
