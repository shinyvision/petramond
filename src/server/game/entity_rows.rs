//! Interest-scoped entity rows: each tick window refreshes every session's
//! [`EntityInterest`](super::interest::EntityInterest), builds the row of
//! every entity SOMEBODY tracks exactly once, and cuts each recipient's
//! lanes as selections over those shared tables.

use std::sync::Arc;

use crate::events::tick::TickEvents;
use crate::mob::riding::MountTarget;
use crate::net::protocol::{
    EntityLane, ItemLane, ItemStateRow, MobLane, MobStateRow, PlayerActionKind, PlayerLane,
    PlayerStateRow, RowSet, Transform,
};
use crate::player::PlayerId;
use petramond_world::inventory::Hand;

use super::interest::{
    view_blocks, LaneIndex, LaneSelection, Viewpoint, ITEM_TRACKING_BLOCKS, MOB_TRACKING_BLOCKS,
};
use super::ServerGame;

/// One recipient's entity replication for a tick window.
pub struct RecipientEntities {
    pub mobs: MobLane,
    pub items: ItemLane,
    pub players: PlayerLane,
    pub player_actions: Arc<[(PlayerId, PlayerActionKind)]>,
}

/// One lane's rows for a window: only the entities some recipient tracks, in
/// world order, plus where each index-order entity landed (`u32::MAX` =
/// tracked by nobody, never built).
struct RowTable<R> {
    rows: Arc<[R]>,
    slot: Vec<u32>,
}

impl<R> RowTable<R> {
    fn build(
        len: usize,
        tracked: impl Iterator<Item = u32>,
        mut row: impl FnMut(usize) -> R,
    ) -> Self {
        const UNTRACKED: u32 = u32::MAX;
        let mut slot = vec![UNTRACKED; len];
        for i in tracked {
            slot[i as usize] = 0;
        }
        let mut rows = Vec::new();
        for (i, s) in slot.iter_mut().enumerate() {
            if *s != UNTRACKED {
                *s = rows.len() as u32;
                rows.push(row(i));
            }
        }
        RowTable {
            rows: rows.into(),
            slot,
        }
    }

    fn lane<K>(&self, sel: LaneSelection<K>) -> EntityLane<R, K> {
        let picks = |indices: Vec<u32>| {
            let picks = indices.iter().map(|&i| self.slot[i as usize]).collect();
            RowSet::select(Arc::clone(&self.rows), picks)
        };
        EntityLane {
            despawned: sel.despawned,
            spawned: picks(sel.spawned),
            updated: picks(sel.updated),
        }
    }
}

impl ServerGame {
    /// Advance every session's interest to this window and build each
    /// recipient's lanes (indexed like `sessions`). A session always tracks
    /// itself, and a tracked rider always brings its mount.
    pub(super) fn entity_lanes(&mut self, events: &TickEvents) -> Vec<RecipientEntities> {
        let mobs = self.world.mobs().instances();
        let mob_index = LaneIndex::new(mobs.iter().map(|m| (m.id(), m.pos)));
        let item_index =
            LaneIndex::new(self.world.item_entities().iter().map(|it| (it.id, it.pos)));
        let player_index = LaneIndex::new(self.sessions.iter().map(|s| (s.id, s.player.pos)));
        let ridden: Vec<Option<u32>> = self
            .sessions
            .iter()
            .map(|sess| match sess.sim.mount?.target {
                MountTarget::Mob(id) => mob_index.position_of(id),
                MountTarget::Anchor(_) => None,
            })
            .collect();
        // Interest never reaches past what the server streams at all.
        let view_cap = self.world.data().render_dist;
        let selections: Vec<_> = self
            .sessions
            .iter_mut()
            .enumerate()
            .map(|(s, sess)| {
                let view_blocks = view_blocks(sess.transport.view_radius, view_cap);
                let at = sess.player.pos;
                let players = sess.transport.interest.players.refresh(
                    &player_index,
                    Viewpoint::new(at, view_blocks),
                    &[s as u32],
                );
                let mounts: Vec<u32> = players
                    .tracked()
                    .filter_map(|p| ridden[p as usize])
                    .collect();
                let mobs = sess.transport.interest.mobs.refresh(
                    &mob_index,
                    Viewpoint::new(at, view_blocks.min(MOB_TRACKING_BLOCKS)),
                    &mounts,
                );
                let items = sess.transport.interest.items.refresh(
                    &item_index,
                    Viewpoint::new(at, view_blocks.min(ITEM_TRACKING_BLOCKS)),
                    &[],
                );
                (mobs, items, players)
            })
            .collect();

        let now = self.world.current_tick();
        let mobs = self.world.mobs().instances();
        let mob_table = RowTable::build(
            mobs.len(),
            selections.iter().flat_map(|(m, _, _)| m.tracked()),
            |i| mob_row(&mobs[i], now),
        );
        let items = self.world.item_entities();
        let item_table = RowTable::build(
            items.len(),
            selections.iter().flat_map(|(_, it, _)| it.tracked()),
            |i| item_row(&items[i]),
        );
        let player_table = RowTable::build(
            self.sessions.len(),
            selections.iter().flat_map(|(_, _, p)| p.tracked()),
            |s| self.player_row(s, events),
        );
        let mut actions = Vec::new();
        for s in 0..self.sessions.len() {
            super::player_actions::player_action_kinds(events.player_at(s), |kind| {
                actions.push((s, kind))
            });
        }
        selections
            .into_iter()
            .map(|(mobs, items, players)| {
                let mut in_view = vec![false; self.sessions.len()];
                for p in players.tracked() {
                    in_view[p as usize] = true;
                }
                let player_actions = actions
                    .iter()
                    .filter(|(s, _)| in_view[*s])
                    .map(|&(s, kind)| (self.sessions[s].id, kind))
                    .collect();
                RecipientEntities {
                    mobs: mob_table.lane(mobs),
                    items: item_table.lane(items),
                    players: player_table.lane(players),
                    player_actions,
                }
            })
            .collect()
    }

    /// Session `s`'s player row. `hurt_recent` ships the damage EDGE —
    /// sessions track no hurt timer; each client runs its own flash envelope.
    fn player_row(&self, s: usize, events: &TickEvents) -> PlayerStateRow {
        let sess = &self.sessions[s];
        let alive = sess.player.health() > 0;
        PlayerStateRow {
            conditions: super::replication::condition_stages(sess.player.conditions()),
            id: sess.id,
            transform: Transform {
                pos: sess.player.pos,
                vel: sess.player.vel,
                yaw: sess.player.yaw,
                pitch: sess.player.pitch,
            },
            on_ground: sess.player.on_ground,
            sneaking: sess.sneaking(),
            sleeping: sess.sim.sleep.is_some(),
            sleep_yaw: self.sleep_head_yaw(s),
            alive,
            visible: alive && !sess.player.is_spectator(),
            held_item: sess.selected_item().map(|item| item.0),
            held_data: sess
                .player
                .inventory
                .selected()
                .and_then(|st| petramond_world::item::variant::blob(st.variant))
                .map(|b| (*b).clone()),
            off_hand_item: sess.player.inventory.off_hand().map(|st| st.item.0),
            off_hand_data: sess
                .player
                .inventory
                .off_hand()
                .and_then(|st| petramond_world::item::variant::blob(st.variant))
                .map(|b| (*b).clone()),
            // The same overlay state `SelfState::mining` ships for the
            // player's own hand: target cell + crack stage. Observers derive
            // the arm-swing flag AND the remote crack overlay.
            mining: sess.sim.mining.overlay(),
            eating: sess.sim.eating.is_some(),
            eating_off_hand: sess
                .sim
                .eating
                .as_ref()
                .is_some_and(|eat| eat.hand == Hand::Off),
            // Mod-set held-item poses (`SetPlayerHeldPose`), already resolved
            // across mods — this is what makes a raised guard visible on
            // somebody ELSE's body.
            held_pose_main: sess.player.claims.held_pose(Hand::Main),
            held_pose_off: sess.player.claims.held_pose(Hand::Off),
            held_display: [
                sess.player.claims.held_display(Hand::Main).map(|i| i.0),
                sess.player.claims.held_display(Hand::Off).map(|i| i.0),
            ],
            bone_poses: sess.player.claims.bone_poses().collect(),
            animator: {
                let mut claims = sess.player.claims.animator().clone();
                claims.retain_observed();
                claims
            },
            hurt_recent: events.player_at(s).player_damaged,
            snap: sess.replication.tick_teleported,
            mount: sess
                .sim
                .mount
                .map(crate::net::protocol::PlayerMount::from_mount),
        }
    }
}

fn mob_row(m: &crate::mob::Instance, now: u64) -> MobStateRow {
    MobStateRow {
        id: m.id(),
        kind_id: m.kind.0,
        pos: m.pos,
        yaw: m.yaw,
        tilt: m.tilt,
        anim_time: m.anim_time,
        moving: m.moving,
        idle_anim: m.idle_anim,
        head_yaw: m.head_yaw,
        head_pitch: m.head_pitch,
        hurt_timer: m.hurt_timer(),
        dead: m.is_dead(),
        shorn: m.is_shorn(),
        emitters: m.active_emitters().to_vec(),
        conditions: super::replication::condition_stages(m.exposure().conditions()),
        anims: m
            .active_anims()
            .iter()
            .map(|l| (l.name.clone(), l.phase))
            .collect(),
        // Present only while the death ragdoll plays (bounded): the pose as
        // of this tick (alpha 1.0); the client interpolates consecutive
        // batches.
        ragdoll: m.ragdoll_pose(1.0).map(|pose| {
            pose.into_iter()
                .map(|(p, q)| (p.to_array(), q.to_array()))
                .collect()
        }),
        dig: m.dig_overlay(now),
        held: m.held().map(|item| item.map(|item| item.0)),
        draw: m.draw().clone(),
    }
}

fn item_row(it: &crate::entity::DroppedItem) -> ItemStateRow {
    ItemStateRow {
        id: it.id,
        item_id: it.stack.item.0,
        count: it.stack.count,
        data: petramond_world::item::variant::blob(it.stack.variant).map(|b| (*b).clone()),
        pos: it.pos,
        spin: it.spin,
        flight: it.heading().map(|h| [h.yaw, h.pitch, it.vel.length()]),
    }
}
