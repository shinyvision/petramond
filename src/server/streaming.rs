//! Server-side world streaming + per-connection terrain replication
//! (multiplayer Phase C2c-ii).
//!
//! Each pump the server streams its OWN world around every session's player
//! (`update_load_multi` + `poll` + `pump_light_bakes` — the drive that used to
//! live on the client's per-frame `tick_world`), then diffs each connection's
//! WANTED terrain shape against what it was already sent and emits
//! `ColumnData`/`SectionData`/`SectionUnload`/`ColumnUnload` messages. Over
//! the in-process pipe the payloads are `Arc` refcount bumps.
//!
//! The wanted/keep shapes are the streamer's own (`World::plan_terrain_send`
//! reuses `column_wanted`/`column_kept` over the anchor's facing target), and
//! the diff is INCREMENTAL: it reruns only when the anchor's quantized target
//! or the world's terrain-content revision moved (`World::terrain_send_key`),
//! or while a previous plan hit the per-pump budget.

use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::IVec3;
use crate::net::protocol::{SectionCacheClaim, ServerToClient, SECTION_CACHE_CAP};
use crate::world::LoadAnchor;
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

/// Loopback has no RTT to hide, so one smaller batch is enough. The single
/// in-flight window is the hard bound on the unbounded process-local channel.
const LOCAL_MAX_BATCH_MSGS: usize = 64;

/// Unacknowledged batches allowed in flight after the FIRST ack proves the
/// client speaks the ack loop; exactly one before it (vanilla-verified
/// 1.20.2 values — see WIKI/multiplayer.md "prior art").
const MAX_UNACKED_BATCHES: u32 = 4;
const LOCAL_MAX_UNACKED_BATCHES: u32 = 1;

/// The assumed client apply rate (streaming messages/second) before the
/// first ack reports a measured one. Modest on purpose: it sizes only the
/// first window-of-one batch.
const INITIAL_CLIENT_RATE: f32 = 1600.0;

/// Server-side clamp on the client-reported rate: a hostile or broken ack
/// can neither park streaming near zero nor open the throttle unboundedly
/// (the queue-headroom allowance still backstops the high end).
const CLIENT_RATE_BOUNDS: (f32, f32) = (50.0, 50_000.0);

/// Outbound-queue slots the streamer must always leave free for tick
/// updates, unloads, and broadcasts. Below this headroom a remote session
/// ships NO streaming messages this pump (terrain AND light — light defers
/// through `TerrainSync::pending_light`) and retries once the writer drains —
/// a full queue disconnects the client (`TcpServerConn::send`), so streaming,
/// which the SERVER rate-controls, must never be what fills it.
const STREAM_QUEUE_RESERVE: usize = crate::net::connection::SERVER_QUEUE_MSGS / 4;

/// Messages this pump may queue for a connection with `queue_room` free
/// outbound slots. The local pipe passes `usize::MAX` (unbounded channel).
fn stream_allowance(queue_room: usize) -> usize {
    queue_room.saturating_sub(STREAM_QUEUE_RESERVE)
}

/// How many sections to PLAN for what remains of a pump's allowance after
/// light: the flat cap, shrunk so the emitted messages (each section plus
/// its possible column refresh = up to two) fit. Unload messages pay from
/// the same allowance during emission, which clips the batch and re-plans —
/// this is only the plan-size heuristic, not the pacing itself.
#[cfg(test)]
fn terrain_budget(allowance: usize) -> usize {
    TERRAIN_SECTIONS_PER_PUMP.min(allowance / 2)
}

/// One connection's terrain replication state: what it currently holds, plus
/// the ack-windowed flow-control state for remote connections.
pub(crate) struct TerrainSync {
    sent_columns: FxHashSet<ChunkPos>,
    sent_column_revisions: FxHashMap<ChunkPos, u64>,
    sent_sections: FxHashSet<SectionPos>,
    /// Sent sections whose fresh server bake is still unshipped — the
    /// per-connection carryover when a pump's allowance ran out (the ship log
    /// itself is drained once, globally). Payloads are fetched at SHIP time,
    /// so rebakes landing while a section waits here coalesce into one
    /// message. Entries leave with their section's unload.
    pending_light: FxHashSet<SectionPos>,
    /// The `terrain_send_key` the last executed diff ran under.
    last_send_key: Option<u64>,
    /// The last diff hit the section budget: keep scanning next pump.
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
    /// Batches sent but not yet `StreamBatchAck`ed. At `max_unacked` the
    /// streamer sends NOTHING further — a slow client means send slower,
    /// never kick.
    unacked_batches: u32,
    /// The in-flight window: 1 until the first ack proves the ack loop, then the
    /// connection's configured cap (still 1 for loopback).
    max_unacked: u32,
    /// The client's last reported apply rate (messages/second), clamped to
    /// [`CLIENT_RATE_BOUNDS`]; [`INITIAL_CLIENT_RATE`] before the first ack.
    client_rate: f32,
    /// Fractional message budget banked from `client_rate × dt` each pump
    /// (capped at one max batch); a batch spends its message count from it.
    batch_quota: f32,
    batch_limit: usize,
    window_limit: u32,
}

impl Default for TerrainSync {
    fn default() -> Self {
        TerrainSync {
            sent_columns: FxHashSet::default(),
            sent_column_revisions: FxHashMap::default(),
            sent_sections: FxHashSet::default(),
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
            batch_quota: 0.0,
            batch_limit: MAX_BATCH_MSGS,
            window_limit: MAX_UNACKED_BATCHES,
        }
    }
}

impl TerrainSync {
    /// Whether the cell's owning section was sent to this connection — the
    /// per-recipient block-delta filter.
    pub(crate) fn covers(&self, pos: IVec3) -> bool {
        SectionPos::from_world(pos.x, pos.y, pos.z)
            .is_some_and(|sp| self.sent_sections.contains(&sp))
    }

    /// Apply one `StreamBatchAck`: retire a batch from the window, widen it to
    /// this connection's configured cap, and adopt the client's
    /// measured rate (clamped; non-finite reports are ignored entirely —
    /// a NaN must never poison the quota).
    pub(crate) fn apply_batch_ack(&mut self, messages_per_second: f32) {
        self.unacked_batches = self.unacked_batches.saturating_sub(1);
        self.max_unacked = self.window_limit;
        if messages_per_second.is_finite() {
            self.client_rate =
                messages_per_second.clamp(CLIENT_RATE_BOUNDS.0, CLIENT_RATE_BOUNDS.1);
        }
    }

    /// Believe the client parks `sp` under `hash` (it was just vouched in an
    /// unload, or claimed in the Join manifest).
    fn note_client_cached(&mut self, sp: SectionPos, hash: u64) {
        let stamp = self.client_cache_stamp;
        self.client_cache_stamp += 1;
        self.client_cache.insert(sp, (hash, stamp));
        if self.client_cache.len() > SECTION_CACHE_CAP {
            // O(cap) u64 scan per over-cap insert, mirroring the client.
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

    /// Seed the belief map from a Join manifest, in claim order (the client
    /// emits claims oldest-first, keeping the shared eviction order intact).
    /// Claims cost nothing to believe: a stale or fabricated one either
    /// hash-mismatches into an ordinary full send or heals through
    /// `SectionCacheMiss`.
    pub(crate) fn seed_client_cache(&mut self, claims: &[SectionCacheClaim]) {
        for claim in claims.iter().take(SECTION_CACHE_CAP) {
            self.note_client_cached(claim.pos, claim.hash);
        }
    }

    /// Apply one `SectionCacheMiss`: the client could not honor a
    /// `SectionCached` re-promotion, so the pos never landed. Forget the
    /// belief and, if we had marked it sent, queue it for an ordinary full
    /// send on the next pump.
    pub(crate) fn handle_cache_miss(&mut self, pos: SectionPos) {
        self.client_cache.remove(&pos);
        if self.sent_sections.remove(&pos) {
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
    /// Every session's streaming anchor: the player's eye section.
    fn load_anchors(&self) -> Vec<LoadAnchor> {
        self.sessions
            .iter()
            .map(|sess| {
                let eye = sess.player.eye();
                LoadAnchor {
                    cx: (eye.x.floor() as i32).div_euclid(16),
                    cy: (eye.y.floor() as i32).div_euclid(16),
                    cz: (eye.z.floor() as i32).div_euclid(16),
                    // The session's requested view distance; world/streaming
                    // clamp it to the server budget (`world.render_dist`).
                    radius: sess.view_radius,
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
        // Emission runs BEFORE the world's own streaming step: a moved
        // anchor's plan already drops terrain against the NEW target, and the
        // world — whose `unload_far` runs the same kept shape — has not
        // evicted it yet, so unload vouching (`section_payload` at the drop)
        // still has content to hash. The step's own products just ship one
        // pump (~5 ms) later: sections it ingests via the next plan, light it
        // bakes via the ship log drained here next pump.
        let relit = self.world.take_light_ship_log();
        let local_at_zero = self.has_local_session;
        for (s, msgs) in per_session.iter_mut().enumerate() {
            self.bank_light_refreshes(s, &relit);
            self.sessions[s]
                .terrain
                .configure_loopback(s == 0 && local_at_zero);
            self.send_batch_for(s, anchors[s], dt, queue_room[s], msgs);
        }

        self.world.update_load_multi(&anchors);
        let _ = self.world.poll();
        // Headless worlds drain (and request — see `headless_relight`) their
        // light bakes here; nothing else pumps them without a mesh queue.
        self.world.pump_light_bakes();
    }

    /// Emit at most one ack-windowed streaming batch for remote session `s`:
    /// nothing while the window is full; otherwise light + terrain up to
    /// `min(banked quota, queue headroom allowance)` messages, bracketed by
    /// `StreamBatchStart`/`StreamBatchEnd{count}`. The quota banks
    /// `client_rate × dt` per pump (capped at one max batch) so the send
    /// rate tracks what the client MEASURED itself applying; the headroom
    /// allowance stays as the transport backstop. An empty emission sends no
    /// markers and consumes no window.
    fn send_batch_for(
        &mut self,
        s: usize,
        anchor: LoadAnchor,
        dt: f32,
        queue_room: usize,
        msgs: &mut Vec<ServerToClient>,
    ) {
        let sync = &mut self.sessions[s].terrain;
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
        let sync = &mut self.sessions[s].terrain;
        sync.batch_quota -= count as f32;
        sync.unacked_batches += 1;
    }

    /// Record this pump's freshly-baked sections into the sessions's pending
    /// carryover. Sections it never received are skipped — their eventual
    /// `SectionData` carries current light.
    fn bank_light_refreshes(&mut self, s: usize, relit: &[SectionPos]) {
        let sync = &mut self.sessions[s].terrain;
        for &sp in relit {
            if sync.sent_sections.contains(&sp) {
                sync.pending_light.insert(sp);
            }
        }
    }

    /// Ship `LightData` for pending refreshed sections, up to `allowance`
    /// messages; the remainder stays in `pending_light`.
    fn send_light_for(&mut self, s: usize, allowance: &mut usize, msgs: &mut Vec<ServerToClient>) {
        let sync = &mut self.sessions[s].terrain;
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
            // Evicted server-side: nothing to ship (the recipient's copy
            // unloads through the terrain diff).
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
        let sync = &mut self.sessions[s].terrain;
        let plan_empty = sync.planned_sections.is_empty()
            && sync.planned_drop_sections.is_empty()
            && sync.planned_drop_columns.is_empty();
        let target_changed = sync.planned_target_key != Some(target_key);
        if target_changed || (plan_empty && (sync.last_send_key != Some(key) || sync.backlog)) {
            let plan = self.world.plan_terrain_send(
                anchor,
                &sync.sent_columns,
                &sync.sent_sections,
                usize::MAX,
            );
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
            // Vouch the implicitly dropped live sections for the client's
            // cache, cy-ascending — the order the client parks them in, so
            // both caps keep evicting the same entries. No vouching for a
            // section the server world already evicted (nothing to hash) or
            // with an unshipped rebake in `pending_light` (the client's
            // light is stale; a re-promotion would resurrect it as current).
            let mut dropped: Vec<SectionPos> = sync
                .sent_sections
                .iter()
                .filter(|sp| sp.chunk_pos() == cp)
                .copied()
                .collect();
            dropped.sort_unstable_by_key(|sp| sp.cy);
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
            sync.sent_sections.retain(|sp| sp.chunk_pos() != cp);
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
            let column_revision = self.world.column_payload_revision(cp);
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
            sync.sent_sections.remove(&sp);
            // Same vouching rules as the column-drop loop above.
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
            let column_revision = self.world.column_payload_revision(cp);
            let fresh_column =
                sync.sent_column_revisions.get(&cp).copied() != Some(column_revision);
            if *allowance < 1 + usize::from(fresh_column) {
                break;
            }
            sync.planned_sections.pop_front();
            if fresh_column {
                let Some(column) = self.world.column_payload(cp) else {
                    continue; // column evicted mid-plan: skip its sections too
                };
                sync.sent_columns.insert(cp);
                sync.sent_column_revisions.insert(cp, column_revision);
                *allowance -= 1;
                msgs.push(ServerToClient::ColumnData(column));
            }
            let Some(section) = self.world.section_payload(sp) else {
                continue;
            };
            sync.sent_sections.insert(sp);
            *allowance -= 1;
            // A section the client holds cached with unmoved content ships
            // as a tiny re-promotion instead of the full payload. Either
            // branch consumes the belief: after this message the section is
            // live client-side (or a full send superseded the parked copy).
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
mod tests {
    use super::*;
    use crate::net::connection::SERVER_QUEUE_MSGS;
    use crate::net::protocol::ClientToServer;
    use crate::server::game::PumpOutput;
    use crate::server::player::PlayerId;
    use std::time::{Duration, Instant};

    fn count_terrain(msgs: &[ServerToClient]) -> usize {
        msgs.iter()
            .filter(|m| {
                matches!(
                    m,
                    ServerToClient::ColumnData(_) | ServerToClient::SectionData(_)
                )
            })
            .count()
    }

    /// What a live client does: answer every `StreamBatchEnd` with an ack.
    /// Feed the result into the next pump's inbox.
    fn acks(out: &PumpOutput) -> Vec<(PlayerId, ClientToServer)> {
        out.remote
            .iter()
            .flat_map(|(id, msgs)| {
                msgs.iter()
                    .filter(|m| matches!(m, ServerToClient::StreamBatchEnd { .. }))
                    .map(move |_| {
                        (
                            *id,
                            ClientToServer::StreamBatchAck {
                                messages_per_second: 1e9, // server clamps
                            },
                        )
                    })
            })
            .collect()
    }

    fn batch_markers(msg: &ServerToClient) -> bool {
        matches!(
            msg,
            ServerToClient::StreamBatchStart | ServerToClient::StreamBatchEnd { .. }
        )
    }

    /// The allowance shuts off below the reserve and the section budget never
    /// exceeds the flat cap (the local pipe's `usize::MAX` room included) —
    /// the two edges that keep pacing from either starving a healthy client
    /// or overflowing a slow one.
    #[test]
    fn stream_allowance_pauses_below_the_reserve_and_terrain_stays_capped() {
        assert_eq!(stream_allowance(0), 0);
        assert_eq!(stream_allowance(STREAM_QUEUE_RESERVE), 0);
        assert!(stream_allowance(STREAM_QUEUE_RESERVE + 64) > 0);
        assert_eq!(terrain_budget(0), 0);
        assert_eq!(
            terrain_budget(stream_allowance(SERVER_QUEUE_MSGS)),
            TERRAIN_SECTIONS_PER_PUMP
        );
        assert_eq!(
            terrain_budget(stream_allowance(usize::MAX)),
            TERRAIN_SECTIONS_PER_PUMP
        );
    }

    /// A remote session whose outbound queue reports no room gets NOTHING but
    /// tick updates — no terrain, no light — while other recipients keep
    /// streaming, and the pause must not mark the paused session's diff as
    /// done: once room returns, the withheld terrain ships. This is the
    /// anti-kick contract — streaming paces itself to the connection instead
    /// of overflowing its bounded queue.
    #[test]
    fn starved_sessions_pause_streaming_and_resume_without_losing_any() {
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let player = crate::game::session::spawn_player(server.world.seed);
        let s = server.add_session_for_test(player);
        let remote_id = server.sessions[s].id;

        // Starve the remote queue until the LOCAL session has received
        // terrain — the world demonstrably had shippable sections, and the
        // starved session got only tick updates.
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut local_terrain = 0usize;
        while local_terrain == 0 {
            assert!(Instant::now() < deadline, "no terrain became shippable");
            let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, 0)]);
            local_terrain += count_terrain(&out.msgs);
            for (id, msgs) in &out.remote {
                assert_eq!(*id, remote_id);
                assert!(
                    msgs.iter().all(|m| matches!(m, ServerToClient::Tick(_))),
                    "a zero-headroom session receives only tick updates"
                );
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        // Room returns: the withheld terrain ships to the remote session.
        let mut remote_terrain = 0usize;
        let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
        while remote_terrain == 0 {
            assert!(Instant::now() < deadline, "paused terrain never resumed");
            let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
            remote_terrain += out
                .remote
                .iter()
                .map(|(_, msgs)| count_terrain(msgs))
                .sum::<usize>();
            inbox = acks(&out);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The ack window (1.20.2 design): exactly ONE batch ships before the
    /// first ack; a client that stops acking gets tick updates and nothing
    /// else (sent LESS, never kicked); an ack reopens the window and widens
    /// it, so streaming resumes.
    #[test]
    fn stream_batches_window_on_acks_and_stall_without_them() {
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let player = crate::game::session::spawn_player(server.world.seed);
        let s = server.add_session_for_test(player);
        let remote_id = server.sessions[s].id;

        // Pump WITHOUT acking until the first batch lands.
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut batches = 0usize;
        while batches == 0 {
            assert!(Instant::now() < deadline, "no first batch");
            let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, SERVER_QUEUE_MSGS)]);
            batches += out
                .remote
                .iter()
                .flat_map(|(_, msgs)| msgs)
                .filter(|m| matches!(m, ServerToClient::StreamBatchStart))
                .count();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(batches, 1, "the pre-ack window is exactly one batch");

        // Still no acks: the window is full — only tick updates flow.
        for _ in 0..50 {
            let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, SERVER_QUEUE_MSGS)]);
            for (_, msgs) in &out.remote {
                assert!(
                    msgs.iter().all(|m| matches!(m, ServerToClient::Tick(_))),
                    "an unacked window ships nothing but tick updates"
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        // One ack: the window reopens and the next batch ships.
        let mut inbox = vec![(
            remote_id,
            ClientToServer::StreamBatchAck {
                messages_per_second: 1e9,
            },
        )];
        let mut resumed = 0usize;
        while resumed == 0 {
            assert!(Instant::now() < deadline, "streaming never resumed on ack");
            let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
            resumed += out
                .remote
                .iter()
                .flat_map(|(_, msgs)| msgs)
                .filter(|m| matches!(m, ServerToClient::StreamBatchStart))
                .count();
            inbox = acks(&out);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Unload bursts pace like everything else: a sweep dropping thousands
    /// of a session's sent sections at once (server-side eviction, keep-shape
    /// exit) must clip to the per-pump allowance instead of overflowing the
    /// queue — and every clipped unload must still arrive, re-found by later
    /// plans' diffs, because a lost unload leaks replica memory forever.
    #[test]
    fn unload_bursts_clip_to_the_allowance_and_all_arrive() {
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let player = crate::game::session::spawn_player(server.world.seed);
        let s = server.add_session_for_test(player);
        let remote_id = server.sessions[s].id;

        // Fake a big sent set far outside any keep shape: the next executed
        // plan wants ALL of it dropped at once.
        let mut awaiting: FxHashSet<SectionPos> = (0..3000)
            .map(|i| SectionPos::new(1000 + i, 0, 1000))
            .collect();
        server.sessions[s]
            .terrain
            .sent_sections
            .extend(awaiting.iter().copied());
        server.sessions[s].terrain.backlog = true;

        let room = STREAM_QUEUE_RESERVE + 100;
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
        while !awaiting.is_empty() {
            assert!(Instant::now() < deadline, "clipped unloads never drained");
            let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, room)]);
            for (_, msgs) in &out.remote {
                let streamed = msgs
                    .iter()
                    .filter(|m| !matches!(m, ServerToClient::Tick(_)) && !batch_markers(m))
                    .count();
                assert!(
                    streamed <= 100,
                    "one pump emitted {streamed} streaming messages into 100 \
                     slots of allowance"
                );
                for m in msgs {
                    if let ServerToClient::SectionUnload { pos, .. } = m {
                        awaiting.remove(pos);
                    }
                }
            }
            inbox = acks(&out);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Light refreshes defer per connection: a rebake landing while a
    /// session's queue is starved must reach it AFTER the queue drains (the
    /// global ship log is drained once per pump — without the carryover the
    /// refresh would be lost and the replica's light permanently stale).
    #[test]
    fn light_refreshes_defer_for_starved_sessions_and_ship_later() {
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let player = crate::game::session::spawn_player(server.world.seed);
        let s = server.add_session_for_test(player);
        let remote_id = server.sessions[s].id;

        // Stream normally (acking like a live client) until the remote
        // session holds a lit section.
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut lit: Option<SectionPos> = None;
        let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
        while lit.is_none() {
            assert!(Instant::now() < deadline, "no lit section streamed");
            let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
            lit = out.remote.iter().flat_map(|(_, msgs)| msgs).find_map(|m| {
                let ServerToClient::SectionData(p) = m else {
                    return None;
                };
                p.skylight.is_some().then_some(p.pos)
            });
            inbox = acks(&out);
            std::thread::sleep(Duration::from_millis(2));
        }
        let lit = lit.unwrap();

        // Dirty the section's light (a solid block changes the sky column),
        // then pump the remote at ZERO headroom until the rebake lands in its
        // pending carryover — the refresh exists, the starved session got no
        // message for it.
        let (bx, by, bz) = (lit.cx * 16 + 8, lit.cy * 16 + 8, lit.cz * 16 + 8);
        assert!(
            server
                .world
                .set_block_world(bx, by, bz, crate::block::Block::Stone),
            "edit lands inside the streamed section"
        );
        while !server.sessions[s].terrain.pending_light.contains(&lit) {
            assert!(Instant::now() < deadline, "the rebake never landed");
            let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, 0)]);
            assert!(
                out.remote
                    .iter()
                    .flat_map(|(_, msgs)| msgs)
                    .all(|m| { !matches!(m, ServerToClient::LightData(_)) }),
                "a zero-headroom session receives no light refreshes"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        // The queue drains: the deferred refresh ships.
        let mut remote_relit = false;
        let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
        while !remote_relit {
            assert!(
                Instant::now() < deadline,
                "the deferred refresh never shipped"
            );
            let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
            remote_relit = out
                .remote
                .iter()
                .flat_map(|(_, msgs)| msgs)
                .any(|m| matches!(m, ServerToClient::LightData(p) if p.pos == lit));
            inbox = acks(&out);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
