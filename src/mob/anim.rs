use mod_api::{MAX_MOB_ANIM_NAME_BYTES, MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE};
use petramond_world::bbmodel::clips;

use super::brain::{BehaviorOutput, HeadLook};
use super::instance::Instance;
use super::kinematics::turn_toward;
use super::MobDef;

const HEAD_TURN_RATE: f32 = 9.0;
const HEAD_SMOOTH_TIME: f32 = 0.16;
const HEAD_SETTLED: f32 = 1.0e-3;

fn smooth_step(to: f32, vel: &mut f32, smooth_time: f32, max_speed: f32, dt: f32) -> f32 {
    let omega = 2.0 / smooth_time;
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let limit = max_speed * smooth_time;
    let change = (-to).clamp(-limit, limit);
    let temp = (*vel + omega * change) * dt;
    *vel = (*vel - omega * temp) * decay;
    let moved = (change + temp) * decay - change;
    if moved.abs() >= to.abs() || (to.abs() < HEAD_SETTLED && vel.abs() < HEAD_SETTLED * 10.0) {
        *vel = 0.0;
        return to;
    }
    moved
}

#[derive(Clone, Debug, Default)]
pub(super) struct Expression {
    pub idle_anim: Option<u8>,
    pub animation: Option<String>,
    pub head_look: Option<HeadLook>,
}

impl From<BehaviorOutput> for Expression {
    fn from(decision: BehaviorOutput) -> Self {
        Expression {
            idle_anim: decision.idle_anim,
            animation: decision.animation,
            head_look: decision.head_look,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum AnimKind {
    Walk,
    Idle(u8),
    Rest,
}

#[derive(Clone, Debug)]
pub struct AnimLayer {
    pub name: String,
    pub phase: f32,
    pub rate: f32,
    pub seek: Option<f32>,
}

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
    #[inline]
    pub fn active_anims(&self) -> &[AnimLayer] {
        &self.presentation.active_anims
    }

    pub(super) fn set_anim_active(&mut self, name: &str, active: bool) -> bool {
        if name.len() > MAX_MOB_ANIM_NAME_BYTES {
            return false;
        }
        match (self.anim_search(name), active) {
            (Ok(_), true) | (Err(_), false) => true,
            (Ok(at), false) => {
                self.presentation.active_anims.remove(at);
                true
            }
            (Err(at), true) => {
                if self.presentation.active_anims.len() >= super::MAX_ACTIVE_MOB_ANIMS {
                    return false;
                }
                self.presentation.active_anims.insert(
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

    pub(super) fn anim_state(&self, name: &str) -> Option<&AnimLayer> {
        if name.len() > MAX_MOB_ANIM_NAME_BYTES {
            return None;
        }
        self.anim_search(name)
            .ok()
            .map(|at| &self.presentation.active_anims[at])
    }

    fn anim_search(&self, name: &str) -> Result<usize, usize> {
        self.presentation
            .active_anims
            .binary_search_by(|a| a.name.as_str().cmp(name))
    }

    fn active_anim_mut(&mut self, name: &str) -> Option<&mut AnimLayer> {
        let at = self.anim_search(name).ok()?;
        Some(&mut self.presentation.active_anims[at])
    }

    pub(super) fn apply_expression(
        &mut self,
        dt: f32,
        d: &MobDef,
        named_anims: &[super::model_meta::NamedAnimMeta],
        decision: &Expression,
    ) {
        if let Ok(at) = named_anims.binary_search_by(|m| m.name.as_str().cmp(clips::AMBIENT)) {
            let meta = &named_anims[at];
            if meta.looping
                && meta.length > 0.0
                && self.anim_state(clips::AMBIENT).is_none()
                && self.set_anim_active(clips::AMBIENT, true)
            {
                let phase = phase_stagger(self.id) * meta.length;
                if let Some(layer) = self.active_anim_mut(clips::AMBIENT) {
                    layer.phase = phase;
                }
            }
        }
        self.idle_anim = if self.moving {
            None
        } else {
            decision.idle_anim
        };

        // Pick the active animation and reset its phase whenever it changes.
        let kind = if self.moving {
            AnimKind::Walk
        } else if let Some(i) = self.idle_anim {
            AnimKind::Idle(i)
        } else {
            AnimKind::Rest
        };
        if kind != self.presentation.anim_kind {
            self.presentation.anim_kind = kind;
            self.anim_time = 0.0;
            self.interp.anim_time = 0.0;
        }
        // An upward launch from walking re-phases the walk clip FORWARD to the next cycle
        // boundary. A repeating launch period near the clip length drifts a free-running clock
        // into anti-phase, tucking legs at takeoff and kicking at landing.
        // Never reset to 0: the replica interpolates raw prev→curr snapshots, so a backward jump
        // would sweep the clip in reverse for a frame on clients.
        // A walk's first launch already starts at phase 0 from the kind-change reset above.
        if std::mem::take(&mut self.motion.walk_launch) && kind == AnimKind::Walk {
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
        match kind {
            AnimKind::Walk => {
                self.anim_time +=
                    d.walk_anim_rate * self.motion.walk_speed_scale * self.motion.gait_pace * dt
            }
            AnimKind::Idle(_) => self.anim_time += dt,
            AnimKind::Rest => {}
        }
        for layer in &mut self.presentation.active_anims {
            step_anim_layer(layer, dt);
        }
        self.presentation.active_anims.retain(|layer| {
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

        let (target_yaw, target_pitch) = match decision.head_look {
            Some(h) => (h.yaw, h.pitch),
            None => (0.0, 0.0),
        };
        let yaw_left = turn_toward(self.head_yaw, target_yaw, std::f32::consts::PI) - self.head_yaw;
        self.head_yaw += smooth_step(
            yaw_left,
            &mut self.presentation.head_vel[0],
            HEAD_SMOOTH_TIME,
            HEAD_TURN_RATE,
            dt,
        );
        self.head_pitch += smooth_step(
            target_pitch - self.head_pitch,
            &mut self.presentation.head_vel[1],
            HEAD_SMOOTH_TIME,
            HEAD_TURN_RATE,
            dt,
        );
    }
}

fn phase_stagger(id: u64) -> f32 {
    const FIBONACCI_HASH: u64 = 0x9e37_79b9_7f4a_7c15;
    let hashed = id.wrapping_mul(FIBONACCI_HASH);
    (hashed >> 40) as f32 / (1u32 << 24) as f32
}

pub fn valid_clip_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_MOB_ANIM_NAME_BYTES
}

#[cfg(test)]
mod tests;
