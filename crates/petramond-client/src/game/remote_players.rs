//! Client-side REMOTE-PLAYER store: every OTHER
//! connected player's replicated rows plus the per-remote presentation state
//! that animates their body.
//!
//! Fed by the per-tick [`TickUpdate`](petramond::net::protocol::TickUpdate)
//! batches like the mob/item stores (`game/replicated.rs`): prev/curr row
//! pairs interpolate at `tick_alpha`, absent ids drop, `snap` rows skip
//! interpolation (tick-side teleports). On top of the rows each remote owns
//! the SAME drivers the local player uses — the shared [`BodyPose`] (walk
//! cycle + body-yaw follow), an eased held view per hand, and the two hand
//! FRAMES its renderer-owned body animator reads — advanced once per frame
//! in `Game::tick_receive`, so a remote's mining loop, fired gestures, and
//! chew read identically to the local body's.
//!
//! Approximations (deliberate, documented):
//! - EATING replicates as a level bool; the frame wants an `Option<f32>`
//!   progress, so a client-side ramp (`EAT_RAMP_SECS`) stands in.
//! - HURT replicates as the `hurt_recent` edge (sessions track no timer); the
//!   client runs its own linear flash envelope, mirroring the local body's
//!   hurt-flash (the app's hurt-shake envelope, 0.25 s).

use std::collections::BTreeMap;

use petramond::net::protocol::{PlayerActionKind, PlayerStateRow};
use petramond::player::{AnimatorClock, AnimatorPlay, PlayerId, RigId};
use petramond_render::{HeldItemEase, HeldItemFrame, HeldItemView};

use super::body_pose::{lerp_angle, BodyPose, MovementMedium};

/// Seconds the remote hurt flash lasts — mirrors the LOCAL third-person
/// body's flash envelope (`app::HURT_SHAKE_SECS`, linear).
const HURT_FLASH_SECS: f32 = 0.25;
/// Client-side stand-in for the replicated-as-bool eat progress: foods take a
/// few seconds, and the ramp only paces what a body graph reads from `eat`.
const EAT_RAMP_SECS: f32 = 3.0;
/// How far back a scrubbed play's progress may step and still ease rather
/// than snap: further back is a restart, and easing across it would play the
/// clip backward.
const SCRUB_RESTART: f32 = 0.25;

/// One remote player: the interpolation row pair plus per-remote animation
/// state.
pub struct RemotePlayer {
    pub prev: PlayerStateRow,
    pub curr: PlayerStateRow,
    /// The shared body pose (walk cycle + body-yaw follow) — the same helper
    /// the local third-person view drives.
    pub pose: BodyPose,
    /// Each hand's eased held view (`[main, off]`).
    ease: [HeldItemEase; 2],
    /// Graph events the batches fired on this body's rigs (the engine's
    /// gestures and mod fires alike) since the last frame — the remote twin
    /// of the App's `hand_events` latch.
    pending: Vec<(RigId, u16)>,
    /// Remaining hurt-flash seconds (see [`HURT_FLASH_SECS`]).
    hurt_t: f32,
    /// Client-side eat-progress ramp (see [`EAT_RAMP_SECS`]).
    eat_t: f32,
    /// The main hand's eased held view this frame — what presentation
    /// attaches to the posed hand.
    pub view: HeldItemView,
    /// The LEFT hand's.
    pub off_view: HeldItemView,
    /// This body's eased bone offsets, so its bones move at the same rate as
    /// the item in its fist. Holds the eased value between frames; the
    /// presentation gather copies it into this frame's arena.
    pub bones: super::bone_ease::BoneEase,
    /// Scratch for this body's resolved offset target, reused across frames.
    target: Vec<petramond_render::BoneOffset>,
    /// This frame's two hand frames (`[main, off]`), which the body's
    /// renderer-owned animator reads.
    pub frames: [HeldItemFrame; 2],
    /// This frame's claimed plays (see [`present_plays`]), sorted like the
    /// rows by rig and slot.
    pub plays: Vec<AnimatorPlay>,
    /// Last frame's plays, reused as this frame's scratch.
    last_plays: Vec<AnimatorPlay>,
    /// The graph events fired on this body this frame, once each.
    pub events: Vec<(RigId, u16)>,
}

/// `plays`' entry for `play`'s rig, slot and clip, walking `cursor` forward:
/// both lists are sorted by rig and slot.
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

impl RemotePlayer {
    fn new(row: PlayerStateRow) -> Self {
        let mut pose = BodyPose::default();
        pose.reset_facing(row.transform.yaw);
        Self {
            prev: row.clone(),
            curr: row,
            pose,
            ease: Default::default(),
            pending: Vec::new(),
            hurt_t: 0.0,
            eat_t: 0.0,
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

    /// The hurt-flash intensity `[0, 1]` for this frame (linear decay).
    pub fn hurt_flash01(&self) -> f32 {
        (self.hurt_t / HURT_FLASH_SECS).clamp(0.0, 1.0)
    }

    /// This remote's soft push body at its last-batch position, or `None` when
    /// there is nothing to jostle: spectators and the dead ship
    /// `visible = false`, a sleeping body is tucked in a bed it must not
    /// be shoved off, and a MOUNTED body is slaved to its seat (shoving it —
    /// or being shoved by it — would fight the mount glue every frame).
    /// Consumed by the local player's per-frame entity push
    /// (`Game::apply_entity_push`) through the same body separation rule
    /// mobs use.
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

/// The client's remote-player set. `BTreeMap` so presentation iterates in a
/// deterministic (id) order, like the other replicated stores.
#[derive(Default)]
pub struct RemotePlayers {
    map: BTreeMap<PlayerId, RemotePlayer>,
}

impl RemotePlayers {
    /// Apply one batch: a known id shifts curr→prev and adopts the new row
    /// (`snap` rows adopt into BOTH so no frame interpolates across the
    /// teleport, and the pose re-faces the landing yaw); a fresh id starts
    /// with prev == curr; an id absent from the batch dropped (left). The
    /// recipient's OWN id is skipped entirely — the local body renders from
    /// the existing predicted-player path.
    pub fn apply(
        &mut self,
        players: &[PlayerStateRow],
        actions: &[(PlayerId, PlayerActionKind)],
        self_id: PlayerId,
    ) {
        let mut old = std::mem::take(&mut self.map);
        for row in players {
            if row.id == self_id {
                continue;
            }
            let mut entry = old
                .remove(&row.id)
                .unwrap_or_else(|| RemotePlayer::new(row.clone()));
            entry.prev = if row.snap {
                row.clone()
            } else {
                entry.curr.clone()
            };
            if row.snap {
                entry.pose.reset_facing(row.transform.yaw);
            }
            entry.curr = row.clone();
            if row.hurt_recent {
                entry.hurt_t = HURT_FLASH_SECS;
            }
            self.map.insert(row.id, entry);
        }
        // `Died`/`Respawned` need no edge: the `visible` flag and `snap`
        // carry their presentation.
        for (id, kind) in actions {
            if let PlayerActionKind::Animator { rig, event } = *kind {
                if let Some(p) = self.map.get_mut(id) {
                    p.pending.push((rig, event));
                }
            }
        }
    }

    /// One frame of presentation state for every remote: the shared body pose
    /// from the interpolated speed/yaw at `alpha`, the hand frames from the
    /// replicated flags, this frame's plays and fired events, the hurt-flash
    /// and eat ramps. Runs in `Game::tick_receive` after the batches applied.
    pub fn advance(
        &mut self,
        dt: f32,
        alpha: f32,
        medium: impl Fn(petramond_math::world_pos::WorldPos) -> MovementMedium,
    ) {
        let ease = 1.0 - (-petramond_render::POSE_EASE_RATE * dt).exp();
        for p in self.map.values_mut() {
            if p.curr.sleeping {
                // Lying body: head toward the pillow, walk cycle rested —
                // mirrors the local sleep branch.
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
            // One edge per frame: two batches in a window can carry the
            // same gesture twice, and an animator fires a slot once.
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
                    .and_then(petramond_world::item::variant::intern_blob)
                    .unwrap_or_default(),
                // Held-rotation preview state isn't replicated; the default
                // reads fine at held-mini-cube size.
                block_state: Default::default(),
                // The row ships the full overlay (target + stage); the arm
                // swing only needs the level flag.
                mining: p.curr.mining.is_some(),
                eating: eating_main.then_some(p.eat_t),
                pose_target: p.curr.held_pose_main.map(super::render_held_pose),
            };
            p.view = p.ease[0].update(&main_frame, dt);
            // The LEFT hand: its own item, its own eats. Mining is a
            // main-hand level by definition.
            let off_frame = HeldItemFrame {
                item: p.curr.off_hand_item.map(petramond_world::item::ItemType),
                display: p.curr.held_display[1].map(petramond_world::item::ItemType),
                variant: p
                    .curr
                    .off_hand_data
                    .as_deref()
                    .and_then(petramond_world::item::variant::intern_blob)
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
            // The body's bones ease at the same rate as the item in its
            // fist, so a raised guard and the arm raising it arrive together.
            let mut target = std::mem::take(&mut p.target);
            target.clear();
            super::render_bone_offsets(&p.curr.bone_poses, &mut target);
            p.bones.advance(&target, dt);
            p.target = target;
            p.hurt_t = (p.hurt_t - dt).max(0.0);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &RemotePlayer> {
        self.map.values()
    }

    /// [`iter`](Self::iter) with each remote's id — for consumers that key
    /// per-player state on it (the footstep cadence).
    pub fn iter_with_ids(&self) -> impl Iterator<Item = (PlayerId, &RemotePlayer)> {
        self.map.iter().map(|(id, p)| (*id, p))
    }

    /// How many remotes exist / are asleep, for the sleep overlay's
    /// "x/y players sleeping" line.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sleeping_count(&self) -> usize {
        self.map.values().filter(|p| p.curr.sleeping).count()
    }
}

/// Interpolate a remote's transform between two batches: position lerps,
/// yaw takes the shortest arc, pitch lerps. A `snap` row was applied with
/// prev == curr, so this is the identity across a teleport.
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
