//! Server-side world streaming + per-connection terrain replication.
//!
//! Each pump the server streams its OWN world around every session's player
//! (`update_load_multi` + `poll` + `pump_light_bakes`), then diffs each connection's
//! WANTED terrain shape against what it was already sent and emits
//! `ColumnData`/`SectionData`/`SectionUnload`/`ColumnUnload` messages. Over
//! the in-process pipe the payloads are `Arc` refcount bumps.
//!
//! The wanted/keep shapes are the streamer's own (`World::plan_terrain_send`
//! reuses `column_wanted`/`column_kept` over the anchor's facing target), and
//! the diff is INCREMENTAL: it reruns only when the anchor's quantized target
//! or the world's terrain-content revision moved (`World::terrain_send_key`),
//! or while a previous plan hit the per-pump budget.

use crate::net::protocol::{SectionCacheClaim, ServerToClient, SECTION_CACHE_CAP};
use crate::world::{LoadAnchor, SentSections};
use petramond_math::math::IVec3;
use petramond_world::chunk::{ChunkPos, SectionPos};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use super::game::ServerGame;

/// Sections considered per pump per connection. The message-batch allowance is
/// normally the tighter bound.
#[cfg(test)]
const TERRAIN_SECTIONS_PER_PUMP: usize = 128;

/// Remote batches stay small enough to apply inside one client frame even when
/// section reconstruction is expensive. Several may be in flight to cover RTT.
const MAX_BATCH_MSGS: usize = 96;

/// Two-deep window lets server bank the next batch while client applies the current one (acks land
/// once per client frame). At 64 msgs/batch loopback delivery capped at ~3.8k sections/s, below
/// RD32 sprint-flight demand (~5k/s), and replica coverage collapsed under load. Wider window
/// raises the ceiling above demand; pacing stays client-driven. Window is still the hard bound on
/// the unbounded channel.
///
/// 192 (was 96): loopback batches use `Arc` payloads and are cheap to apply, so the remote
/// per-frame sizing doesn't bind here. Spawn's 3x3 columns are ~2x96 msgs, so the wider batch
/// lands them in one frame instead of trickling.
const LOCAL_MAX_BATCH_MSGS: usize = 192;

const MAX_UNACKED_BATCHES: u32 = 4;
const LOCAL_MAX_UNACKED_BATCHES: u32 = 2;

const INITIAL_CLIENT_RATE: f32 = 1600.0;

const CLIENT_RATE_BOUNDS: (f32, f32) = (50.0, 50_000.0);

/// Outbound-queue slots the streamer must always leave free for tick
/// updates, unloads, and broadcasts. Below this headroom a remote session
/// ships NO streaming messages this pump (terrain AND light — light defers
/// through `TerrainSync::pending_light`) and retries once the writer drains —
/// a full queue disconnects the client (`TcpServerConn::send`), so streaming,
/// which the SERVER rate-controls, must never be what fills it.
const STREAM_QUEUE_RESERVE: usize = crate::net::connection::SERVER_QUEUE_MSGS / 4;

fn stream_allowance(queue_room: usize) -> usize {
    queue_room.saturating_sub(STREAM_QUEUE_RESERVE)
}

#[cfg(test)]
fn terrain_budget(allowance: usize) -> usize {
    TERRAIN_SECTIONS_PER_PUMP.min(allowance / 2)
}

pub struct TerrainSync {
    sent_columns: FxHashSet<ChunkPos>,
    sent_column_revisions: FxHashMap<ChunkPos, u64>,
    sent: SentSections,
    pending_light: FxHashSet<SectionPos>,
    last_send_key: Option<u64>,
    backlog: bool,
    /// One full nearest-first diff, consumed incrementally across paced batches.
    /// World revisions that arrive while this plan is non-empty are folded into
    /// the next refill instead of rescanning every 5 ms.
    planned_sections: VecDeque<SectionPos>,
    planned_drop_sections: VecDeque<SectionPos>,
    planned_drop_columns: VecDeque<ChunkPos>,
    planned_target_key: Option<u64>,
    /// What this connection's client holds in its SECTION CACHE, by the
    /// server-domain content hash vouched at unload (value: hash + insertion
    /// stamp). Seeded from the Join manifest, grown by unload emission,
    /// consumed by the `SectionCached`-vs-`SectionData` decision. Capped at
    /// [`SECTION_CACHE_CAP`] with oldest-first eviction — the client runs the
    /// same policy over the same ordered unload stream, so the two maps stay
    /// aligned without eviction chatter; residual drift (a client that
    /// declined to park, a divergent claim) heals through `SectionCacheMiss`.
    client_cache: FxHashMap<SectionPos, (u64, u64)>,
    client_cache_stamp: u64,
    unacked_batches: u32,
    max_unacked: u32,
    client_rate: f32,
    /// Fractional message budget banked from `client_rate × dt` each pump
    /// (capped at one max batch); a batch spends its message count from it.
    /// Starts FULL: a joining connection gets one max-size opening batch
    /// (still window-bounded to a single in-flight batch until the first
    /// ack) instead of trickling `INITIAL_CLIENT_RATE × dt` messages per
    /// pump while the player stares at an empty spawn.
    batch_quota: f32,
    batch_limit: usize,
    window_limit: u32,
}

impl Default for TerrainSync {
    fn default() -> Self {
        TerrainSync {
            sent_columns: FxHashSet::default(),
            sent_column_revisions: FxHashMap::default(),
            sent: SentSections::default(),
            pending_light: FxHashSet::default(),
            last_send_key: None,
            backlog: false,
            planned_sections: VecDeque::new(),
            planned_drop_sections: VecDeque::new(),
            planned_drop_columns: VecDeque::new(),
            planned_target_key: None,
            client_cache: FxHashMap::default(),
            client_cache_stamp: 0,
            unacked_batches: 0,
            max_unacked: 1,
            client_rate: INITIAL_CLIENT_RATE,
            batch_quota: MAX_BATCH_MSGS as f32,
            batch_limit: MAX_BATCH_MSGS,
            window_limit: MAX_UNACKED_BATCHES,
        }
    }
}

impl TerrainSync {
    pub fn covers(&self, pos: IVec3) -> bool {
        SectionPos::from_world(pos.x, pos.y, pos.z).is_some_and(|sp| self.sent.contains(sp))
    }

    pub fn apply_batch_ack(&mut self, messages_per_second: f32) {
        self.unacked_batches = self.unacked_batches.saturating_sub(1);
        self.max_unacked = self.window_limit;
        if messages_per_second.is_finite() {
            self.client_rate =
                messages_per_second.clamp(CLIENT_RATE_BOUNDS.0, CLIENT_RATE_BOUNDS.1);
        }
    }

    fn note_client_cached(&mut self, sp: SectionPos, hash: u64) {
        let stamp = self.client_cache_stamp;
        self.client_cache_stamp += 1;
        self.client_cache.insert(sp, (hash, stamp));
        if self.client_cache.len() > SECTION_CACHE_CAP {
            if let Some(oldest) = self
                .client_cache
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(p, _)| *p)
            {
                self.client_cache.remove(&oldest);
            }
        }
    }

    pub fn seed_client_cache(&mut self, claims: &[SectionCacheClaim]) {
        for claim in claims.iter().take(SECTION_CACHE_CAP) {
            self.note_client_cached(claim.pos, claim.hash);
        }
    }

    pub fn handle_cache_miss(&mut self, pos: SectionPos) {
        self.client_cache.remove(&pos);
        if self.sent.remove(pos) {
            self.pending_light.remove(&pos);
            self.planned_sections.push_back(pos);
        }
    }

    fn configure_loopback(&mut self, loopback: bool) {
        let (batch_limit, window_limit) = if loopback {
            (LOCAL_MAX_BATCH_MSGS, LOCAL_MAX_UNACKED_BATCHES)
        } else {
            (MAX_BATCH_MSGS, MAX_UNACKED_BATCHES)
        };
        self.batch_limit = batch_limit;
        self.window_limit = window_limit;
        self.max_unacked = self.max_unacked.min(window_limit).max(1);
        self.batch_quota = self.batch_quota.min(batch_limit as f32);
    }
}

impl ServerGame {
    #[cfg(any(test, feature = "test-support"))]
    pub fn mark_section_sent_for_test(&mut self, session: usize, cell: IVec3) {
        let section = SectionPos::from_world(cell.x, cell.y, cell.z).expect("valid test cell");
        self.sessions[session]
            .transport
            .terrain
            .sent
            .insert(section);
    }

    fn load_anchors(&self) -> Vec<LoadAnchor> {
        self.sessions
            .iter()
            .map(|sess| {
                let eye = sess.player.eye();
                LoadAnchor {
                    cx: (eye.x.floor() as i32).div_euclid(16),
                    cy: (eye.y.floor() as i32).div_euclid(16),
                    cz: (eye.z.floor() as i32).div_euclid(16),
                    radius: sess.transport.view_radius,
                }
            })
            .collect()
    }

    /// One pump's streaming step: drive the server world's own streaming, then
    /// emit terrain messages for EVERY session — each connection diffs its own
    /// wanted shape through its `TerrainSync`. `per_session` and `queue_room`
    /// (free outbound-queue slots per connection; `usize::MAX` = unbounded)
    /// are indexed like `sessions` (the pump built them that way); `dt` is
    /// the pump's wall-clock step, feeding the batch quota.
    ///
    /// Refreshes are BANKED first (against the pre-terrain sent set — a
    /// section terrain ships this same pump carries current light in its own
    /// payload, so it must not double-ship) but EMITTED last: terrain owns
    /// the allowance. During a load, seam rebakes outnumber new sections
    /// ~2:1, and light-first spent most of a saturated writer's throughput
    /// correcting light on terrain the client already had while the world
    /// itself trickled. Deferring is also cheaper in total: a frontier
    /// section rebakes several times as its neighbours land, and pending
    /// entries fetch their payload at SHIP time, so those rebakes coalesce
    /// into one message once the writer frees up.
    pub(super) fn pump_streaming(
        &mut self,
        dt: f32,
        per_session: &mut [Vec<ServerToClient>],
        queue_room: &[usize],
    ) {
        debug_assert_eq!(per_session.len(), self.sessions.len());
        debug_assert_eq!(queue_room.len(), self.sessions.len());
        let anchors = self.load_anchors();
        if anchors.is_empty() {
            return;
        }
        // Emission runs before world's streaming step. Moved anchor's plan drops terrain against
        // the new target, but unload_far hasn't evicted it yet, so unload vouching still has
        // content to hash. World's own products (sections, relit chunks) land one pump later.
        let relit = self.world.take_light_ship_log();
        let local_at_zero = self.sessions.has_local_session();
        for (s, msgs) in per_session.iter_mut().enumerate() {
            self.bank_light_refreshes(s, &relit);
            self.sessions[s]
                .transport
                .terrain
                .configure_loopback(s == 0 && local_at_zero);
            self.send_batch_for(s, anchors[s], dt, queue_room[s], msgs);
        }

        self.world.update_load_multi(&anchors);
        let _ = self.world.poll();
        self.world.pump_light_bakes();
    }

    fn send_batch_for(
        &mut self,
        s: usize,
        anchor: LoadAnchor,
        dt: f32,
        queue_room: usize,
        msgs: &mut Vec<ServerToClient>,
    ) {
        let sync = &mut self.sessions[s].transport.terrain;
        sync.batch_quota =
            (sync.batch_quota + sync.client_rate * dt.max(0.0)).min(sync.batch_limit as f32);
        if sync.unacked_batches >= sync.max_unacked {
            return;
        }
        let quota = sync.batch_quota as usize;
        let mut allowance = quota
            .min(sync.batch_limit)
            .min(stream_allowance(queue_room));
        if allowance == 0 {
            return;
        }
        let start = msgs.len();
        self.send_terrain_for(s, anchor, &mut allowance, msgs);
        self.send_light_for(s, &mut allowance, msgs);
        let count = msgs.len() - start;
        if count == 0 {
            return;
        }
        msgs.insert(start, ServerToClient::StreamBatchStart);
        msgs.push(ServerToClient::StreamBatchEnd {
            count: count as u32,
        });
        let sync = &mut self.sessions[s].transport.terrain;
        sync.batch_quota -= count as f32;
        sync.unacked_batches += 1;
    }

    fn bank_light_refreshes(&mut self, s: usize, relit: &[SectionPos]) {
        let sync = &mut self.sessions[s].transport.terrain;
        for &sp in relit {
            if sync.sent.contains(sp) {
                sync.pending_light.insert(sp);
            }
        }
    }

    fn send_light_for(&mut self, s: usize, allowance: &mut usize, msgs: &mut Vec<ServerToClient>) {
        let sync = &mut self.sessions[s].transport.terrain;
        if sync.pending_light.is_empty() {
            return;
        }
        let batch: Vec<SectionPos> = sync
            .pending_light
            .iter()
            .take(*allowance)
            .copied()
            .collect();
        for sp in batch {
            sync.pending_light.remove(&sp);
            let Some(p) = self.world.light_payload(sp) else {
                continue;
            };
            *allowance -= 1;
            msgs.push(ServerToClient::LightData(p));
        }
    }

    /// Diff session `s`'s wanted terrain against its sent sets and append the
    /// resulting messages: unloads first, then each new section preceded by
    /// its (re-freshed) column payload — column-before-section is the install
    /// contract, and re-shipping the column keeps the replica's heightmap and
    /// summaries current as more of the column lands server-side.
    ///
    /// EVERY emitted message pays from `allowance`, unloads included — a
    /// server-side eviction sweep can drop thousands of sent sections at
    /// once, and an unpaced unload burst overflows the connection queue just
    /// like unpaced terrain did. Deferring emission is always safe: the sent
    /// sets are updated ONLY for messages actually emitted, so the next
    /// plan's diff re-finds whatever was clipped (`backlog` forces that
    /// replan). A zero-allowance pump skips WITHOUT touching
    /// `last_send_key` — a key is only ever marked done by a plan that ran
    /// under it — so paused streaming always resumes.
    fn send_terrain_for(
        &mut self,
        s: usize,
        anchor: LoadAnchor,
        allowance: &mut usize,
        msgs: &mut Vec<ServerToClient>,
    ) {
        if *allowance == 0 {
            return;
        }
        let key = self.world.terrain_send_key(anchor);
        let target_key = self.world.terrain_target_key(anchor);
        let sync = &mut self.sessions[s].transport.terrain;
        let plan_empty = sync.planned_sections.is_empty()
            && sync.planned_drop_sections.is_empty()
            && sync.planned_drop_columns.is_empty();
        let target_changed = sync.planned_target_key != Some(target_key);
        if target_changed || (plan_empty && (sync.last_send_key != Some(key) || sync.backlog)) {
            let plan =
                self.world
                    .plan_terrain_send(anchor, &sync.sent_columns, &sync.sent, usize::MAX);
            sync.planned_sections = plan.sections.into();
            sync.planned_drop_sections = plan.drop_sections.into();
            sync.planned_drop_columns = plan.drop_columns.into();
            sync.planned_target_key = Some(target_key);
            sync.last_send_key = Some(key);
        }
        if sync.planned_sections.is_empty()
            && sync.planned_drop_sections.is_empty()
            && sync.planned_drop_columns.is_empty()
        {
            sync.backlog = false;
            return;
        }

        while let Some(cp) = sync.planned_drop_columns.front().copied() {
            if *allowance == 0 {
                break;
            }
            sync.planned_drop_columns.pop_front();
            *allowance -= 1;
            sync.sent_columns.remove(&cp);
            sync.sent_column_revisions.remove(&cp);
            let dropped = sync.sent.take_column(cp);
            let mut cache_hashes = Vec::new();
            for sp in dropped {
                if sync.pending_light.contains(&sp) {
                    continue;
                }
                if let Some(payload) = self.world.section_payload(sp) {
                    let hash = payload.content_hash();
                    sync.note_client_cached(sp, hash);
                    cache_hashes.push((sp.cy, hash));
                }
            }
            sync.pending_light.retain(|sp| sp.chunk_pos() != cp);
            msgs.push(ServerToClient::ColumnUnload {
                pos: cp,
                cache_hashes,
            });
        }
        while sync.planned_drop_columns.is_empty() {
            let Some(sp) = sync.planned_drop_sections.front().copied() else {
                break;
            };
            let cp = sp.chunk_pos();
            let column_revision = self.world.data().column_payload_revision(cp);
            let fresh_column =
                sync.sent_column_revisions.get(&cp).copied() != Some(column_revision);
            if *allowance < 1 + usize::from(fresh_column) {
                break;
            }
            if fresh_column {
                if let Some(column) = self.world.column_payload(cp) {
                    sync.sent_column_revisions.insert(cp, column_revision);
                    *allowance -= 1;
                    msgs.push(ServerToClient::ColumnData(column));
                }
            }
            sync.planned_drop_sections.pop_front();
            *allowance -= 1;
            sync.sent.remove(sp);
            let cache_hash = (!sync.pending_light.remove(&sp))
                .then(|| self.world.section_payload(sp).map(|p| p.content_hash()))
                .flatten();
            if let Some(hash) = cache_hash {
                sync.note_client_cached(sp, hash);
            }
            msgs.push(ServerToClient::SectionUnload {
                pos: sp,
                cache_hash,
            });
        }

        while sync.planned_drop_columns.is_empty() && sync.planned_drop_sections.is_empty() {
            let Some(sp) = sync.planned_sections.front().copied() else {
                break;
            };
            let cp = sp.chunk_pos();
            let column_revision = self.world.data().column_payload_revision(cp);
            let fresh_column =
                sync.sent_column_revisions.get(&cp).copied() != Some(column_revision);
            if *allowance < 1 + usize::from(fresh_column) {
                break;
            }
            sync.planned_sections.pop_front();
            if fresh_column {
                let Some(column) = self.world.column_payload(cp) else {
                    continue;
                };
                sync.sent_columns.insert(cp);
                sync.sent_column_revisions.insert(cp, column_revision);
                *allowance -= 1;
                msgs.push(ServerToClient::ColumnData(column));
            }
            let Some(section) = self.world.section_payload(sp) else {
                continue;
            };
            sync.sent.insert(sp);
            *allowance -= 1;
            if let Some(&(hash, _)) = sync.client_cache.get(&sp) {
                sync.client_cache.remove(&sp);
                if section.content_hash() == hash {
                    msgs.push(ServerToClient::SectionCached { pos: sp, hash });
                    continue;
                }
            }
            msgs.push(ServerToClient::SectionData(Box::new(section)));
        }
        sync.backlog = !sync.planned_sections.is_empty()
            || !sync.planned_drop_sections.is_empty()
            || !sync.planned_drop_columns.is_empty();
    }
}

#[cfg(test)]
mod tests;
