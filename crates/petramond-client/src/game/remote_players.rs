//! Client-side REMOTE-PLAYER store: every OTHER
//! connected player's replicated rows plus the per-remote presentation state
//! that animates their body.
//!
//! Fed by the per-tick [`TickUpdate`](petramond::net::protocol::TickUpdate)
//! batches like the mob/item stores (`game/replicated.rs`): prev/curr row
//! pairs interpolate at `tick_alpha`, absent ids drop, `snap` rows skip
//! interpolation (tick-side teleports). On top of the rows each remote owns
//! the SAME animation drivers the local player uses — the shared
//! [`BodyPose`] (walk cycle + body-yaw follow) and the renderer's
//! [`HeldItemAnimator`] swing state machine — advanced once per frame in
//! `Game::tick_receive`, so a remote's mining loop, break/place jabs, and chew
//! read identically to first person's.
//!
//! Approximations (deliberate, documented):
//! - EATING replicates as a level bool; the animator wants an `Option<f32>`
//!   progress, so a client-side ramp (`EAT_RAMP_SECS`) stands in. Only the
//!   blend/nibble channels pose the third-person body — the progress channel
//!   (`eat_near`) drives a first-person-only camera approach — so the ramp is
//!   visually exact for remote bodies.
//! - HURT replicates as the `hurt_recent` edge (sessions track no timer); the
//!   client runs its own linear flash envelope, mirroring the local body's
//!   hurt-flash (the app's hurt-shake envelope, 0.25 s).

use std::collections::BTreeMap;

use petramond::net::protocol::{PlayerActionKind, PlayerStateRow};
use petramond::player::PlayerId;
use petramond_render::{HeldItemAnimator, HeldItemFrame, HeldItemView};

use super::body_pose::{lerp_angle, BodyPose, MovementMedium};

/// Seconds the remote hurt flash lasts — mirrors the LOCAL third-person
/// body's flash envelope (`app::HURT_SHAKE_SECS`, linear).
const HURT_FLASH_SECS: f32 = 0.25;
/// Client-side stand-in for the replicated-as-bool eat progress: foods take a
/// few seconds; the exact duration only feeds the first-person-only
/// `eat_near` channel, so this never shows on a remote body.
const EAT_RAMP_SECS: f32 = 3.0;

/// One-shot animation triggers latched from the batch's `player_actions`,
/// consumed by the next frame's animator update — the remote twin of the
/// App's `hand` latch (`latch_game_event_hand_triggers`).
#[derive(Copy, Clone, Debug, Default)]
struct ActionLatch {
    swung: bool,
    broke: bool,
    placed: bool,
    /// The place-jab latch for the LEFT hand — the `*Off` action kinds (the
    /// use-click ladder acted from the off-hand).
    placed_off: bool,
}

impl ActionLatch {
    /// Mirror of the local trigger mapping: a break is the full punch; place/
    /// throw/use/interact all play the softer place jab (their `*Off` twins
    /// jab the LEFT hand); an attack swings.
    /// `AteFinished`/`Died`/`Respawned` need no jab — the eat flag, the
    /// `visible` flag, and `snap` carry their presentation.
    fn note(&mut self, kind: PlayerActionKind) {
        match kind {
            PlayerActionKind::Swung => self.swung = true,
            PlayerActionKind::Broke => self.broke = true,
            PlayerActionKind::Placed
            | PlayerActionKind::ThrewItem
            | PlayerActionKind::UsedItem
            | PlayerActionKind::Interacted => self.placed = true,
            PlayerActionKind::PlacedOff
            | PlayerActionKind::UsedItemOff
            | PlayerActionKind::InteractedOff => self.placed_off = true,
            PlayerActionKind::AteFinished
            | PlayerActionKind::AteFinishedOff
            | PlayerActionKind::Died
            | PlayerActionKind::Respawned => {}
        }
    }
}

/// One remote player: the interpolation row pair plus per-remote animation
/// state.
pub struct RemotePlayer {
    pub prev: PlayerStateRow,
    pub curr: PlayerStateRow,
    /// The shared body pose (walk cycle + body-yaw follow) — the same helper
    /// the local third-person view drives.
    pub pose: BodyPose,
    /// The renderer's held-item swing state machine, one per remote, fed from
    /// the replicated flags + latched one-shots.
    animator: HeldItemAnimator,
    /// The LEFT hand's own animator (off-hand jabs + off-hand eats).
    off_animator: HeldItemAnimator,
    latched: ActionLatch,
    /// Remaining hurt-flash seconds (see [`HURT_FLASH_SECS`]).
    hurt_t: f32,
    /// Client-side eat-progress ramp (see [`EAT_RAMP_SECS`]).
    eat_t: f32,
    /// The animator's output for this frame — what presentation attaches to
    /// the posed hand.
    pub view: HeldItemView,
    /// The off-hand animator's output — the LEFT hand's held item.
    pub off_view: HeldItemView,
    /// This body's eased bone offsets, so its bones move at the same rate as
    /// the item in its fist. Holds the eased value between frames; the
    /// presentation gather copies it into this frame's arena.
    pub bones: super::bone_ease::BoneEase,
    /// Scratch for this body's resolved offset target, reused across frames.
    target: Vec<petramond_render::BoneOffset>,
}

impl RemotePlayer {
    fn new(row: PlayerStateRow) -> Self {
        let mut pose = BodyPose::default();
        pose.reset_facing(row.transform.yaw);
        Self {
            prev: row.clone(),
            curr: row,
            pose,
            animator: HeldItemAnimator::default(),
            off_animator: HeldItemAnimator::default(),
            latched: ActionLatch::default(),
            hurt_t: 0.0,
            eat_t: 0.0,
            view: HeldItemView::default(),
            off_view: HeldItemView::default(),
            bones: Default::default(),
            target: Vec::new(),
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
        for (id, kind) in actions {
            if let Some(p) = self.map.get_mut(id) {
                p.latched.note(*kind);
            }
        }
    }

    /// One frame of presentation state for every remote: the shared body pose
    /// from the interpolated speed/yaw at `alpha`, the held-item animator from
    /// the replicated flags + consumed one-shot latches, the hurt-flash and
    /// eat ramps. Runs in `Game::tick_receive` after the batches applied.
    pub fn advance(
        &mut self,
        dt: f32,
        alpha: f32,
        medium: impl Fn(petramond_math::world_pos::WorldPos) -> MovementMedium,
    ) {
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
            let latch = std::mem::take(&mut p.latched);
            let eating_main = p.curr.eating && !p.curr.eating_off_hand;
            let eating_off = p.curr.eating && p.curr.eating_off_hand;
            // A remote body's arm swing comes from its POSE; view bob is a
            // first-person camera effect and has no meaning on one.
            p.view = p.animator.update(HeldItemFrame {
                bob: [0.0, 0.0],
                motion_offset: [0.0; 3],
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
                broke_block: latch.broke,
                placed: latch.placed,
                swung: latch.swung,
                eating: eating_main.then_some(p.eat_t),
                pose_target: p.curr.held_pose_main.map(super::render_held_pose),
                swing_claim: p.curr.motion_claims[0].contains(mod_api::HandMotion::Swing),
                jab_claim: p.curr.motion_claims[0].contains(mod_api::HandMotion::Jab),
                dt,
            });
            // The LEFT hand: its own item, its own jabs, its own eats. Mining
            // and attack swings are main-hand actions by definition.
            p.off_view = p.off_animator.update(HeldItemFrame {
                bob: [0.0, 0.0],
                motion_offset: [0.0; 3],
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
                broke_block: false,
                placed: latch.placed_off,
                swung: false,
                eating: eating_off.then_some(p.eat_t),
                dt,
                pose_target: p.curr.held_pose_off.map(super::render_held_pose),
                swing_claim: p.curr.motion_claims[1].contains(mod_api::HandMotion::Swing),
                jab_claim: p.curr.motion_claims[1].contains(mod_api::HandMotion::Jab),
            });
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
