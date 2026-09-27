//! Client-side REMOTE-PLAYER store: every OTHER
//! connected player's replicated rows plus the per-remote presentation state
//! that animates their body.
//!
//! Fed by the per-tick [`TickUpdate`](petramond::net::protocol::TickUpdate)
//! batches like the mob/item stores (`game/replicated.rs`): prev/curr row
//! pairs interpolate at `tick_alpha`, a remote lives here from its spawn into
//! this client's interest to its despawn (out of view, or left the game),
//! `snap` rows skip interpolation (tick-side teleports). On top of the rows each remote owns
//! the SAME drivers the local player uses — the shared [`BodyPose`] (walk
//! cycle + body-yaw follow), an eased held view per hand, and the two hand
//! FRAMES its body animator (`crate::animation`) reads — advanced once per frame
//! in `Game::tick_receive`, so a remote's mining loop, fired gestures, and
//! chew read identically to the local body's.
//!
//! Approximations (deliberate, documented):
//! - EATING replicates as a level bool; the frame wants an `Option<f32>`
//!   progress, so a client-side ramp (`EAT_RAMP_SECS`) stands in.
//! - HURT replicates as the `hurt_recent` edge (sessions track no timer); the
//!   client runs its own linear flash envelope, mirroring the local body's
//!   hurt-flash (the app's hurt-shake envelope, 0.25 s).

use super::presentation::BodyEmitters;
use super::replicated::{Adopt, AssignFrom, EntityStore, Replica};

use petramond::net::protocol::{PlayerActionKind, PlayerLane, PlayerStateRow};
use petramond::player::{AnimatorClock, AnimatorPlay, PlayerId, RigId};
use petramond_render::{HeldItemEase, HeldItemFrame, HeldItemView};

use super::body_pose::{lerp_angle, BodyPose, MovementMedium};

const HURT_FLASH_SECS: f32 = 0.25;
const EAT_RAMP_SECS: f32 = 3.0;
const SCRUB_RESTART: f32 = 0.25;

pub struct RemotePlayer {
    pub prev: PlayerStateRow,
    pub curr: PlayerStateRow,
    pub pose: BodyPose,
    ease: [HeldItemEase; 2],
    pending: Vec<(RigId, u16)>,
    hurt_t: f32,
    eat_t: f32,
    emitters: BodyEmitters,
    pub view: HeldItemView,
    pub off_view: HeldItemView,
    pub bones: super::bone_ease::BoneEase,
    target: Vec<crate::animation::BoneOffset>,
    pub frames: [HeldItemFrame; 2],
    pub plays: Vec<AnimatorPlay>,
    last_plays: Vec<AnimatorPlay>,
    pub events: Vec<(RigId, u16)>,
}

fn matching<'a>(
    plays: &'a [AnimatorPlay],
    cursor: &mut usize,
    play: &AnimatorPlay,
) -> Option<&'a AnimatorPlay> {
    while plays
        .get(*cursor)
        .is_some_and(|p| (p.rig, p.slot) < (play.rig, play.slot))
    {
        *cursor += 1;
    }
    plays
        .get(*cursor)
        .filter(|p| (p.rig, p.slot, p.clip) == (play.rig, play.slot, play.clip))
}

/// The plays a remote body presents this frame, into `out`: the newest
/// row's (`curr`), each scrubbed play's progress placed at the frame. The
/// row is a tick old when it lands, so a play still climbing is carried
/// forward by `alpha` of the step the last two rows took (clamped to the
/// clip) — a marker a pack lands its hit on must not fire a tick late on
/// every observer. A play with no earlier row, or one that stepped back,
/// eases from last frame's progress by `ease` (snapping on a restart).
fn present_plays(
    prev: &[AnimatorPlay],
    curr: &[AnimatorPlay],
    last: &[AnimatorPlay],
    alpha: f32,
    ease: f32,
    out: &mut Vec<AnimatorPlay>,
) {
    out.clear();
    let (mut in_prev, mut in_last) = (0, 0);
    for play in curr {
        let mut play = *play;
        if let AnimatorClock::Scrub(progress) = play.clock {
            let before = matching(prev, &mut in_prev, &play).and_then(AnimatorPlay::progress);
            let shown = matching(last, &mut in_last, &play).and_then(AnimatorPlay::progress);
            let at = match (before, shown) {
                (Some(before), _) if progress >= before => {
                    (progress + (progress - before) * alpha).min(1.0)
                }
                (_, Some(shown)) if progress >= shown - SCRUB_RESTART => {
                    shown + (progress - shown) * ease
                }
                _ => progress,
            };
            play.clock = AnimatorClock::Scrub(at);
        }
        out.push(play);
    }
}

impl Replica<PlayerStateRow> for RemotePlayer {
    type Ctx = ();

    fn spawn(row: &PlayerStateRow, _: &mut ()) -> Self {
        let mut pose = BodyPose::default();
        pose.reset_facing(row.transform.yaw);
        let mut emitters = BodyEmitters::default();
        emitters.refresh(&[], &row.conditions);
        Self {
            prev: row.clone(),
            curr: row.clone(),
            pose,
            ease: Default::default(),
            pending: Vec::new(),
            hurt_t: if row.hurt_recent {
                HURT_FLASH_SECS
            } else {
                0.0
            },
            eat_t: 0.0,
            emitters,
            view: HeldItemView::default(),
            off_view: HeldItemView::default(),
            bones: Default::default(),
            target: Vec::new(),
            frames: Default::default(),
            plays: Vec::new(),
            last_plays: Vec::new(),
            events: Vec::new(),
        }
    }

    fn advance(&mut self, row: &PlayerStateRow, _: &mut ()) {
        self.adopt(row, false);
    }

    fn reseed(&mut self, row: &PlayerStateRow, _: &mut ()) {
        self.adopt(row, true);
    }

    fn hold(&mut self) {
        self.prev.assign_from(&self.curr);
    }
}

impl RemotePlayer {
    fn adopt(&mut self, row: &PlayerStateRow, snap: bool) {
        std::mem::swap(&mut self.prev, &mut self.curr);
        self.curr.assign_from(row);
        if snap || row.snap {
            self.prev.assign_from(row);
            self.pose.reset_facing(row.transform.yaw);
        }
        if row.hurt_recent {
            self.hurt_t = HURT_FLASH_SECS;
        }
        self.emitters.refresh(&[], &row.conditions);
    }

    pub fn emitters(&self) -> &BodyEmitters {
        &self.emitters
    }

    pub fn hurt_flash01(&self) -> f32 {
        (self.hurt_t / HURT_FLASH_SECS).clamp(0.0, 1.0)
    }

    pub fn push_body(&self) -> Option<petramond_world::body::Body> {
        (self.curr.visible && !self.curr.sleeping && self.curr.mount.is_none()).then(|| {
            petramond_world::body::Body::new(
                self.curr.transform.pos,
                petramond::player::HALF_W,
                petramond::player::HEIGHT,
            )
        })
    }
}

#[derive(Default)]
pub struct RemotePlayers {
    map: EntityStore<PlayerId, RemotePlayer>,
}

impl RemotePlayers {
    /// Apply one lane (see [`EntityStore::apply`]): an update shifts
    /// curr→prev and adopts the new row in place (`snap` rows adopt into BOTH
    /// so no frame interpolates across the teleport, and the pose re-faces
    /// the landing yaw), a spawn starts with prev == curr, a despawn drops
    /// the remote. `resync` snaps every row (a folded backlog). The
    /// recipient's OWN id is skipped entirely — the local body renders from
    /// the existing predicted-player path.
    pub fn apply(&mut self, players: &PlayerLane, self_id: PlayerId, resync: bool) {
        let adopt = if resync {
            Adopt::Reseed
        } else {
            Adopt::Advance
        };
        self.map
            .apply(players, adopt, &mut (), |row| row.id != self_id);
    }

    pub fn queue_actions(&mut self, actions: &[(PlayerId, PlayerActionKind)]) {
        for (id, kind) in actions {
            if let PlayerActionKind::Animator { rig, event } = *kind {
                if let Some(p) = self.map.get_mut(id) {
                    p.pending.push((rig, event));
                }
            }
        }
    }

    #[cfg(test)]
    pub fn apply_snapshot(
        &mut self,
        players: &[PlayerStateRow],
        actions: &[(PlayerId, PlayerActionKind)],
        self_id: PlayerId,
    ) {
        let lane = super::replicated::snapshot_lane(self.map.keys(), players);
        self.apply(&lane, self_id, false);
        self.queue_actions(actions);
    }

    pub fn advance(
        &mut self,
        dt: f32,
        alpha: f32,
        medium: impl Fn(petramond_math::world_pos::WorldPos) -> MovementMedium,
    ) {
        let ease = 1.0 - (-petramond_render::POSE_EASE_RATE * dt).exp();
        for p in self.map.iter_mut() {
            if p.curr.sleeping {
                p.pose.lie(p.curr.sleep_yaw.unwrap_or(p.curr.transform.yaw));
            } else {
                let vel = p.prev.transform.vel.lerp(p.curr.transform.vel, alpha);
                let pos = p.prev.transform.pos.lerp(p.curr.transform.pos, alpha);
                let yaw = lerp_angle(p.prev.transform.yaw, p.curr.transform.yaw, alpha);
                p.pose.advance(
                    dt,
                    super::body_pose::MotionFrame {
                        position: pos,
                        velocity: vel,
                        yaw,
                        grounded: p.curr.on_ground,
                        medium: medium(pos),
                        enabled: p.curr.visible && p.curr.mount.is_none(),
                        sneaking: p.curr.sneaking,
                    },
                );
            }
            p.eat_t = if p.curr.eating {
                (p.eat_t + dt / EAT_RAMP_SECS).min(1.0)
            } else {
                0.0
            };
            p.events.clear();
            for event in p.pending.drain(..) {
                if !p.events.contains(&event) {
                    p.events.push(event);
                }
            }
            let eating_main = p.curr.eating && !p.curr.eating_off_hand;
            let eating_off = p.curr.eating && p.curr.eating_off_hand;
            let main_frame = HeldItemFrame {
                item: p.curr.held_item.map(petramond_world::item::ItemType),
                display: p.curr.held_display[0].map(petramond_world::item::ItemType),
                variant: p
                    .curr
                    .held_data
                    .as_deref()
                    .and_then(|b| petramond_world::item::variant::intern_blob(b).ok())
                    .unwrap_or_default(),
                block_state: Default::default(),
                mining: p.curr.mining.is_some(),
                eating: eating_main.then_some(p.eat_t),
                pose_target: p.curr.held_pose_main.map(super::render_held_pose),
            };
            p.view = p.ease[0].update(&main_frame, dt);
            let off_frame = HeldItemFrame {
                item: p.curr.off_hand_item.map(petramond_world::item::ItemType),
                display: p.curr.held_display[1].map(petramond_world::item::ItemType),
                variant: p
                    .curr
                    .off_hand_data
                    .as_deref()
                    .and_then(|b| petramond_world::item::variant::intern_blob(b).ok())
                    .unwrap_or_default(),
                block_state: Default::default(),
                mining: false,
                eating: eating_off.then_some(p.eat_t),
                pose_target: p.curr.held_pose_off.map(super::render_held_pose),
            };
            p.off_view = p.ease[1].update(&off_frame, dt);
            p.frames = [main_frame, off_frame];
            std::mem::swap(&mut p.plays, &mut p.last_plays);
            present_plays(
                &p.prev.animator.plays,
                &p.curr.animator.plays,
                &p.last_plays,
                alpha,
                ease,
                &mut p.plays,
            );
            let mut target = std::mem::take(&mut p.target);
            target.clear();
            super::render_bone_offsets(&p.curr.bone_poses, &mut target);
            p.bones.advance(&target, dt);
            p.target = target;
            p.hurt_t = (p.hurt_t - dt).max(0.0);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &RemotePlayer> {
        self.map.iter()
    }

    pub fn iter_with_ids(&self) -> impl Iterator<Item = (PlayerId, &RemotePlayer)> {
        self.map.iter_with_ids()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sleeping_count(&self) -> usize {
        self.map.iter().filter(|p| p.curr.sleeping).count()
    }
}

pub fn interpolate(
    prev: &PlayerStateRow,
    curr: &PlayerStateRow,
    alpha: f32,
) -> (petramond_math::world_pos::WorldPos, f32, f32) {
    let (p, c) = (&prev.transform, &curr.transform);
    (
        p.pos.lerp(c.pos, alpha),
        lerp_angle(p.yaw, c.yaw, alpha),
        p.pitch + (c.pitch - p.pitch) * alpha,
    )
}

#[cfg(test)]
mod tests;
