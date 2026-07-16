//! Client-side REMOTE-PLAYER store (multiplayer Phase F): every OTHER
//! connected player's replicated rows plus the per-remote presentation state
//! that animates their body.
//!
//! Fed by the per-tick [`TickUpdate`](crate::net::protocol::TickUpdate)
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
//!   progress, so a client-side ramp ([`EAT_RAMP_SECS`]) stands in. Only the
//!   blend/nibble channels pose the third-person body — the progress channel
//!   (`eat_near`) drives a first-person-only camera approach — so the ramp is
//!   visually exact for remote bodies.
//! - HURT replicates as the `hurt_recent` edge (sessions track no timer); the
//!   client runs its own linear flash envelope, mirroring the local body's
//!   hurt-flash (the app's hurt-shake envelope, 0.25 s).

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;

use crate::net::protocol::{PlayerActionKind, PlayerStateRow};
use crate::render::{HeldItemAnimator, HeldItemFrame, HeldItemView};
use crate::server::player::PlayerId;

use super::body_pose::{lerp_angle, BodyPose};

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
}

impl ActionLatch {
    /// Mirror of the local trigger mapping: a break is the full punch; place/
    /// throw/use/interact all play the softer place jab; an attack swings.
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
            PlayerActionKind::AteFinished
            | PlayerActionKind::Died
            | PlayerActionKind::Respawned => {}
        }
    }
}

/// One remote player: the interpolation row pair plus per-remote animation
/// state.
pub(crate) struct RemotePlayer {
    /// Roster display name (`PlayerJoined`); presentation-only.
    #[allow(dead_code)] // first consumer: nametags (polish backlog)
    pub(crate) name: String,
    pub(crate) prev: PlayerStateRow,
    pub(crate) curr: PlayerStateRow,
    /// The shared body pose (walk cycle + body-yaw follow) — the same helper
    /// the local third-person view drives.
    pub(crate) pose: BodyPose,
    /// The renderer's held-item swing state machine, one per remote, fed from
    /// the replicated flags + latched one-shots.
    animator: HeldItemAnimator,
    latched: ActionLatch,
    /// Remaining hurt-flash seconds (see [`HURT_FLASH_SECS`]).
    hurt_t: f32,
    /// Client-side eat-progress ramp (see [`EAT_RAMP_SECS`]).
    eat_t: f32,
    /// The animator's output for this frame — what presentation attaches to
    /// the posed hand.
    pub(crate) view: HeldItemView,
}

impl RemotePlayer {
    fn new(row: PlayerStateRow) -> Self {
        let mut pose = BodyPose::default();
        pose.reset_facing(row.transform.yaw);
        Self {
            name: String::new(),
            prev: row,
            curr: row,
            pose,
            animator: HeldItemAnimator::default(),
            latched: ActionLatch::default(),
            hurt_t: 0.0,
            eat_t: 0.0,
            view: HeldItemView::default(),
        }
    }

    /// The hurt-flash intensity `[0, 1]` for this frame (linear decay).
    pub(crate) fn hurt_flash01(&self) -> f32 {
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
    pub(crate) fn push_body(&self) -> Option<crate::body::Body> {
        (self.curr.visible && !self.curr.sleeping && self.curr.mount.is_none()).then(|| {
            crate::body::Body::new(
                self.curr.transform.pos,
                crate::player::HALF_W,
                crate::player::HEIGHT,
            )
        })
    }
}

/// The client's remote-player set. `BTreeMap` so presentation iterates in a
/// deterministic (id) order, like the other replicated stores.
#[derive(Default)]
pub(crate) struct RemotePlayers {
    map: BTreeMap<PlayerId, RemotePlayer>,
}

impl RemotePlayers {
    /// Apply one batch: a known id shifts curr→prev and adopts the new row
    /// (`snap` rows adopt into BOTH so no frame interpolates across the
    /// teleport, and the pose re-faces the landing yaw); a fresh id starts
    /// with prev == curr; an id absent from the batch dropped (left). The
    /// recipient's OWN id is skipped entirely — the local body renders from
    /// the existing predicted-player path.
    pub(crate) fn apply(
        &mut self,
        players: &[PlayerStateRow],
        actions: &[(PlayerId, PlayerActionKind)],
        self_id: PlayerId,
        roster: &HashMap<PlayerId, String>,
    ) {
        let mut old = std::mem::take(&mut self.map);
        for row in players {
            if row.id == self_id {
                continue;
            }
            let mut entry = old
                .remove(&row.id)
                .unwrap_or_else(|| RemotePlayer::new(*row));
            entry.prev = if row.snap { *row } else { entry.curr };
            if row.snap {
                entry.pose.reset_facing(row.transform.yaw);
            }
            entry.curr = *row;
            if row.hurt_recent {
                entry.hurt_t = HURT_FLASH_SECS;
            }
            if let Some(name) = roster.get(&row.id) {
                if entry.name != *name {
                    entry.name = name.clone();
                }
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
    pub(crate) fn advance(&mut self, dt: f32, alpha: f32) {
        for p in self.map.values_mut() {
            if p.curr.sleeping {
                // Lying body: head toward the pillow, walk cycle rested —
                // mirrors the local sleep branch.
                p.pose.lie(p.curr.sleep_yaw.unwrap_or(p.curr.transform.yaw));
            } else {
                let vel = p.prev.transform.vel.lerp(p.curr.transform.vel, alpha);
                let hspeed = Vec3::new(vel.x, 0.0, vel.z).length();
                let yaw = lerp_angle(p.prev.transform.yaw, p.curr.transform.yaw, alpha);
                p.pose
                    .advance(dt, hspeed, yaw, p.curr.visible, p.curr.sneaking);
            }
            p.eat_t = if p.curr.eating {
                (p.eat_t + dt / EAT_RAMP_SECS).min(1.0)
            } else {
                0.0
            };
            let latch = std::mem::take(&mut p.latched);
            p.view = p.animator.update(HeldItemFrame {
                item: p.curr.held_item.map(crate::item::ItemType),
                // Held-rotation preview state isn't replicated; the default
                // reads fine at held-mini-cube size.
                block_state: Default::default(),
                // The row ships the full overlay (target + stage); the arm
                // swing only needs the level flag.
                mining: p.curr.mining.is_some(),
                broke_block: latch.broke,
                placed: latch.placed,
                swung: latch.swung,
                eating: p.curr.eating.then_some(p.eat_t),
                dt,
            });
            p.hurt_t = (p.hurt_t - dt).max(0.0);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &RemotePlayer> {
        self.map.values()
    }

    /// How many remotes exist / are asleep, for the sleep overlay's
    /// "x/y players sleeping" line.
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn sleeping_count(&self) -> usize {
        self.map.values().filter(|p| p.curr.sleeping).count()
    }
}

/// Interpolate a remote's transform between two batches: position lerps,
/// yaw takes the shortest arc, pitch lerps. A `snap` row was applied with
/// prev == curr, so this is the identity across a teleport.
pub(crate) fn interpolate(
    prev: &PlayerStateRow,
    curr: &PlayerStateRow,
    alpha: f32,
) -> (Vec3, f32, f32) {
    let (p, c) = (&prev.transform, &curr.transform);
    (
        p.pos.lerp(c.pos, alpha),
        lerp_angle(p.yaw, c.yaw, alpha),
        p.pitch + (c.pitch - p.pitch) * alpha,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::Transform;

    fn row(id: u8, pos: Vec3) -> PlayerStateRow {
        PlayerStateRow {
            id: PlayerId(id),
            transform: Transform {
                pos,
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            on_ground: true,
            sneaking: false,
            sleeping: false,
            sleep_yaw: None,
            alive: true,
            visible: true,
            held_item: None,
            mining: None,
            eating: false,
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    fn apply(store: &mut RemotePlayers, rows: &[PlayerStateRow]) {
        store.apply(rows, &[], PlayerId(0), &HashMap::new());
    }

    #[test]
    fn store_pairs_batches_skips_own_id_and_drops_absent_ids() {
        let mut store = RemotePlayers::default();
        let p1 = Vec3::new(1.0, 70.0, 1.0);
        let p2 = Vec3::new(2.0, 70.0, 1.0);

        // Own id (0) skipped entirely; fresh ids start prev == curr.
        apply(&mut store, &[row(0, p1), row(1, p1), row(2, p1)]);
        assert_eq!(store.len(), 2, "the recipient's own row is never stored");
        let fresh = store.iter().next().expect("id 1 stored");
        assert_eq!(fresh.prev.transform.pos, p1, "a fresh id interpolates from itself");

        apply(&mut store, &[row(1, p2)]);
        assert_eq!(store.len(), 1, "id 2 absent from the batch: dropped");
        let paired = store.iter().next().unwrap();
        assert_eq!(paired.prev.transform.pos, p1, "previous batch became the prev row");
        assert_eq!(paired.curr.transform.pos, p2);
        // Midpoint interpolation over the pair.
        let (mid, _, _) = interpolate(&paired.prev, &paired.curr, 0.5);
        assert_eq!(mid, Vec3::new(1.5, 70.0, 1.0));
    }

    #[test]
    fn interpolation_lerps_yaw_across_the_wrap_seam() {
        use std::f32::consts::PI;
        let mut a = row(1, Vec3::ZERO);
        a.transform.yaw = PI - 0.1;
        let mut b = row(1, Vec3::ZERO);
        b.transform.yaw = -PI + 0.1;
        let (_, yaw, _) = interpolate(&a, &b, 0.5);
        assert!(
            super::super::body_pose::wrap_angle(yaw - PI).abs() < 1e-5,
            "yaw crosses the seam the short way: {yaw}"
        );
    }

    #[test]
    fn snap_rows_skip_interpolation() {
        let mut store = RemotePlayers::default();
        let here = Vec3::new(1.0, 70.0, 1.0);
        let far = Vec3::new(500.0, 90.0, -300.0);
        apply(&mut store, &[row(1, here)]);
        let mut tp = row(1, far);
        tp.snap = true;
        apply(&mut store, &[tp]);
        let p = store.iter().next().unwrap();
        assert_eq!(p.prev.transform.pos, far, "a snap row adopts into BOTH pair slots");
        let (pos, _, _) = interpolate(&p.prev, &p.curr, 0.25);
        assert_eq!(pos, far, "no frame lerps across the teleport");
    }

    #[test]
    fn a_broke_action_latches_exactly_one_animator_jab() {
        let mut store = RemotePlayers::default();
        store.apply(
            &[row(1, Vec3::ZERO)],
            &[(PlayerId(1), PlayerActionKind::Broke)],
            PlayerId(0),
            &HashMap::new(),
        );
        store.advance(1.0 / 60.0, 1.0);
        let s1 = store.iter().next().unwrap().view.swing;
        assert!(s1 > 0.0, "the latched break starts a swing");

        // The latch is consumed: the next frame CONTINUES the same swing
        // (advances forward) rather than restarting a new jab at phase 0.
        store.advance(1.0 / 60.0, 1.0);
        let s2 = store.iter().next().unwrap().view.swing;
        assert!(s2 > s1, "one jab continues, no re-trigger: {s2} vs {s1}");

        // And it completes back to rest within a swing period.
        store.advance(0.5, 1.0);
        assert_eq!(store.iter().next().unwrap().view.swing, 0.0);
    }

    #[test]
    fn hurt_edge_runs_a_decaying_flash_envelope() {
        let mut store = RemotePlayers::default();
        let mut hurt = row(1, Vec3::ZERO);
        hurt.hurt_recent = true;
        apply(&mut store, &[hurt]);
        assert_eq!(store.iter().next().unwrap().hurt_flash01(), 1.0);
        store.advance(HURT_FLASH_SECS * 0.5, 1.0);
        let mid = store.iter().next().unwrap().hurt_flash01();
        assert!(mid > 0.0 && mid < 1.0, "the flash decays: {mid}");
        store.advance(HURT_FLASH_SECS, 1.0);
        assert_eq!(store.iter().next().unwrap().hurt_flash01(), 0.0);
    }
}
